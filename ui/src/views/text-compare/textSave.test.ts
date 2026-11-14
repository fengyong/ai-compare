import { describe, expect, it } from 'vitest';
import { runTextSave, saveRequestsFor, type TextSaveIO } from './textSave';
import type { TextSaveEntry, TextSaveParams, TextSaveResult } from '../../protocol/types';

/**
 * 文本落盘控制器（R3-04）：逐侧显式保存、另存为目标映射、
 * 能力缺失与失败必须可见（部分成功不得被当成全成功）。
 */

const entry = (side: 'left' | 'right', path: string): TextSaveEntry => ({
  side,
  path,
  bytes: 3 as unknown as bigint,
  lineCount: 1,
});

const result = (saved: TextSaveEntry[]): TextSaveResult => ({
  sessionId: 's1',
  saved,
  rev: 1 as unknown as bigint,
  leftModified: false,
  rightModified: false,
});

function recorder(impl?: (p: TextSaveParams) => Promise<TextSaveResult | null>) {
  const calls: TextSaveParams[] = [];
  const io: TextSaveIO = {
    save: async (p) => {
      calls.push(p);
      if (impl) return impl(p);
      return result([entry(p.side as 'left' | 'right', `/tmp/${p.side}.txt`)]);
    },
  };
  return { calls, io };
}

describe('saveRequestsFor：脏侧 → 保存请求', () => {
  it('只列脏侧，顺序左先右后', () => {
    expect(saveRequestsFor({ left: false, right: false })).toEqual([]);
    expect(saveRequestsFor({ left: true, right: false })).toEqual([{ side: 'left' }]);
    expect(saveRequestsFor({ left: false, right: true })).toEqual([{ side: 'right' }]);
    expect(saveRequestsFor({ left: true, right: true })).toEqual([
      { side: 'left' },
      { side: 'right' },
    ]);
  });
});

describe('runTextSave：逐侧落盘', () => {
  it('空请求不发 RPC', async () => {
    const { calls, io } = recorder();
    const out = await runTextSave('s1', [], io);
    expect(calls).toEqual([]);
    expect(out).toEqual({ saved: [], unsupported: false, error: null });
  });

  it('双侧脏：两次 RPC，side 显式，saved 汇总', async () => {
    const { calls, io } = recorder();
    const out = await runTextSave('s1', saveRequestsFor({ left: true, right: true }), io);
    expect(calls).toEqual([
      { sessionId: 's1', side: 'left' },
      { sessionId: 's1', side: 'right' },
    ]);
    expect(out.saved.map((e) => e.side)).toEqual(['left', 'right']);
    expect(out.error).toBeNull();
    expect(out.unsupported).toBe(false);
  });

  it('另存为：目标只挂到对应侧字段', async () => {
    const { calls, io } = recorder();
    await runTextSave('s1', [{ side: 'right', path: '/tmp/out.txt' }], io);
    expect(calls).toEqual([{ sessionId: 's1', side: 'right', rightPath: '/tmp/out.txt' }]);
    const left = recorder();
    await runTextSave('s1', [{ side: 'left', path: '/tmp/out2.txt' }], left.io);
    expect(left.calls).toEqual([{ sessionId: 's1', side: 'left', leftPath: '/tmp/out2.txt' }]);
  });

  it('能力缺失（null）：unsupported=true 且不再写后续侧', async () => {
    const { calls, io } = recorder(async () => null);
    const out = await runTextSave('s1', [{ side: 'left' }, { side: 'right' }], io);
    expect(calls).toHaveLength(1);
    expect(out.unsupported).toBe(true);
    expect(out.saved).toEqual([]);
    expect(out.error).toBeNull();
  });

  it('第二侧失败：error 可见且第一侧已落盘事实保留', async () => {
    const { calls, io } = recorder(async (p) => {
      if (p.side === 'right') throw new Error('磁盘写入失败');
      return result([entry('left', '/tmp/left.txt')]);
    });
    const out = await runTextSave('s1', [{ side: 'left' }, { side: 'right' }], io);
    expect(calls).toHaveLength(2);
    expect(out.error).toBe('磁盘写入失败');
    expect(out.saved.map((e) => e.side)).toEqual(['left']);
    expect(out.unsupported).toBe(false);
  });

  it('第一侧失败：不再尝试第二侧', async () => {
    const { calls, io } = recorder(async () => {
      throw new Error('会话不存在');
    });
    const out = await runTextSave('s1', [{ side: 'left' }, { side: 'right' }], io);
    expect(calls).toHaveLength(1);
    expect(out.error).toBe('会话不存在');
    expect(out.saved).toEqual([]);
  });

  it('非 Error 抛出物也被转成可读信息', async () => {
    const io: TextSaveIO = { save: async () => Promise.reject('裸字符串') };
    const out = await runTextSave('s1', [{ side: 'left' }], io);
    expect(out.error).toBe('裸字符串');
  });
});
