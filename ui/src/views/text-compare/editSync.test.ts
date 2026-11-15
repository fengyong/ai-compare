import { describe, expect, it } from 'vitest';
import { flushEdits, newEditSyncState, type EditSyncIO } from './editSync';
import type { DiffSummary, TextEditOp, TextEditResult } from '../../protocol/types';

/**
 * 编辑同步状态机（R3-18）：队列合并、单飞行、分叉收敛与重放上限。
 * 视图层（TextCompareView）在 node 环境 vitest 下不可渲染，故把这段
 * 最容易出错的逻辑抽到此处做行为验证。
 */

const SUMMARY: DiffSummary = {
  sectionCount: 1,
  equalSections: 1,
  insertSections: 0,
  deleteSections: 0,
  changeSections: 0,
  equalLines: 2,
  insertedLines: 0,
  deletedLines: 0,
};

function result(leftLineCount: number, rightLineCount: number): TextEditResult {
  return {
    sessionId: 's',
    rev: 1,
    leftLineCount,
    rightLineCount,
    sectionCount: 1,
    summary: SUMMARY,
    sections: [],
  };
}

function harness(
  edit: (edits: TextEditOp[], call: number) => Promise<TextEditResult | null>,
  lines: { left: string[]; right: string[] } = { left: ['a', 'b'], right: ['a', 'b'] },
) {
  const calls: TextEditOp[][] = [];
  const adopted: TextEditResult[] = [];
  const box = { fallbacks: 0, diverged: 0 };
  const io: EditSyncIO = {
    edit: async (edits) => {
      calls.push(edits);
      return edit(edits, calls.length);
    },
    lines: () => lines,
    adopt: async (res) => {
      adopted.push(res);
    },
    fallback: async () => {
      box.fallbacks += 1;
    },
    diverged: () => {
      box.diverged += 1;
    },
  };
  return { io, calls, adopted, lines, box };
}

const op = (start: number, text: string): TextEditOp => ({
  side: 'left',
  start,
  end: start + 1,
  text,
});

describe('flushEdits：编辑 → 引擎权威重 diff（R3-18）', () => {
  it('正常路径：发送补丁并采纳引擎结果，不走客户端兜底', async () => {
    const h = harness(async () => result(2, 2));
    const st = newEditSyncState();
    await flushEdits(st, [op(0, 'X')], h.io);
    expect(h.calls).toEqual([[op(0, 'X')]]);
    expect(h.adopted).toHaveLength(1);
    expect(h.box.fallbacks).toBe(0);
    expect(h.box.diverged).toBe(0);
    expect(st.flushing).toBe(false);
    expect(st.queue).toEqual([]);
  });

  it('飞行中入队的编辑由同一循环消费（不并发起第二个循环）', async () => {
    let release!: (v: TextEditResult) => void;
    const gate = new Promise<TextEditResult>((r) => {
      release = r;
    });
    const h = harness(async (_e, call) => (call === 1 ? gate : result(2, 2)));
    const st = newEditSyncState();
    const first = flushEdits(st, [op(0, 'X')], h.io);
    await Promise.resolve();
    expect(st.flushing).toBe(true);
    // 第二批在第一批飞行中入队：应立即返回，不新起循环
    await flushEdits(st, [op(1, 'Y')], h.io);
    expect(h.calls).toHaveLength(1);
    release(result(2, 2));
    await first;
    expect(h.calls).toHaveLength(2);
    expect(h.calls[1]).toEqual([op(1, 'Y')]);
    expect(h.adopted).toHaveLength(2);
    expect(st.flushing).toBe(false);
  });

  it('应答行数与 UI 不符 → 整侧重放一次后收敛', async () => {
    const h = harness(async (_e, call) => (call === 1 ? result(9, 2) : result(2, 2)));
    await flushEdits(newEditSyncState(), [op(0, 'X')], h.io);
    expect(h.calls).toHaveLength(2);
    expect(h.calls[1]).toEqual([
      { side: 'left', start: 0, text: 'a\nb' },
      { side: 'right', start: 0, text: 'a\nb' },
    ]);
    expect(h.adopted).toHaveLength(1);
    expect(h.box.diverged).toBe(0);
    expect(h.box.fallbacks).toBe(0);
  });

  it('RPC 抛错（引擎原子拒绝整批）→ 整侧重放一次后收敛', async () => {
    const h = harness(async (_e, call) => {
      if (call === 1) throw new Error('RPC -32602: 编辑区间越界');
      return result(2, 2);
    });
    await flushEdits(newEditSyncState(), [op(0, 'X')], h.io);
    expect(h.calls).toHaveLength(2);
    expect(h.calls[1][0]).toEqual({ side: 'left', start: 0, text: 'a\nb' });
    expect(h.adopted).toHaveLength(1);
    expect(h.box.diverged).toBe(0);
  });

  it('重放后仍分叉 → 至多一次重放，标注 + 客户端兜底，队列清空（不死循环）', async () => {
    const h = harness(async () => result(9, 9)); // 永远不符
    const st = newEditSyncState();
    await flushEdits(st, [op(0, 'X')], h.io);
    expect(h.calls).toHaveLength(2); // 原始批 + 一次重放，没有第三次
    expect(h.box.diverged).toBe(1);
    expect(h.box.fallbacks).toBe(1);
    expect(h.adopted).toHaveLength(0);
    expect(st.queue).toEqual([]);
    expect(st.flushing).toBe(false);
  }, 3000);

  it('重放后仍抛错 → 同样收敛到兜底（不死循环）', async () => {
    const h = harness(async () => {
      throw new Error('引擎断连');
    });
    const st = newEditSyncState();
    await flushEdits(st, [op(0, 'X')], h.io);
    expect(h.calls).toHaveLength(2);
    expect(h.box.diverged).toBe(1);
    expect(h.box.fallbacks).toBe(1);
    expect(st.flushing).toBe(false);
  }, 3000);

  it('edit 返回 null（能力位消失）→ 客户端兜底且不再发送', async () => {
    const h = harness(async () => null);
    const st = newEditSyncState();
    await flushEdits(st, [op(0, 'X')], h.io);
    expect(h.calls).toHaveLength(1);
    expect(h.box.fallbacks).toBe(1);
    expect(h.box.diverged).toBe(0);
    expect(h.adopted).toHaveLength(0);
    expect(st.flushing).toBe(false);
  });

  it('兜底渲染抛错也要复位 flushing（否则该会话后续编辑全部静默失效）', async () => {
    const h = harness(async () => null);
    h.io.fallback = async () => {
      throw new Error('渲染失败');
    };
    const st = newEditSyncState();
    await expect(flushEdits(st, [op(0, 'X')], h.io)).rejects.toThrow('渲染失败');
    expect(st.flushing).toBe(false);
  });

  it('会话状态相互独立：A 飞行中不阻塞 B', async () => {
    let release!: (v: TextEditResult) => void;
    const gate = new Promise<TextEditResult>((r) => {
      release = r;
    });
    const hA = harness(async () => gate);
    const hB = harness(async () => result(2, 2));
    const stA = newEditSyncState();
    const stB = newEditSyncState();
    const a = flushEdits(stA, [op(0, 'X')], hA.io);
    await Promise.resolve();
    await flushEdits(stB, [op(0, 'Y')], hB.io);
    expect(hB.adopted).toHaveLength(1);
    expect(hA.adopted).toHaveLength(0);
    release(result(2, 2));
    await a;
    expect(hA.adopted).toHaveLength(1);
  });

  it('空 op 批（无实际变化）不发起 RPC', async () => {
    const h = harness(async () => result(2, 2));
    await flushEdits(newEditSyncState(), [], h.io);
    expect(h.calls).toHaveLength(0);
    expect(h.adopted).toHaveLength(0);
  });
});
