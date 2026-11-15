import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type {
  MergeDecideResult,
  MergeOpenResult,
  MergeOutputResult,
  MergeSaveResult,
  MergeSectionsResult,
} from '../protocol/types';

/**
 * MockEngine P7 方法集（同形模拟）：text/merge3/open|sections|decide|output。
 * 与真实引擎 handlers.rs 的四方法同形：open 统计、sections 分批、decide
 * 幂等（rev 只在决策内容变化时递增）、output plain/marker 合成。
 */

const newClient = async () => {
  const engine = new MockEngine();
  const client = new RpcClient(new MockTransport(engine));
  await client.ready;
  return client;
};

async function openMerge3(client: RpcClient, sessionId = 'm3-1'): Promise<MergeOpenResult> {
  return client.request<MergeOpenResult>('text/merge3/open', {
    sessionId,
    base: '/demo/base.txt',
    left: '/demo/left.txt',
    right: '/demo/right.txt',
    options: null,
  });
}

async function sections(
  client: RpcClient,
  sessionId: string,
  offset: number,
  count: number,
): Promise<MergeSectionsResult> {
  return client.request<MergeSectionsResult>('text/merge3/sections', { sessionId, offset, count });
}

async function output(
  client: RpcClient,
  sessionId: string,
  mode: 'plain' | 'marker' | null = null,
  markerStyle: 'merge' | 'diff3' | null = null,
): Promise<MergeOutputResult> {
  return client.request<MergeOutputResult>('text/merge3/output', { sessionId, mode, markerStyle });
}

describe('MockEngine P7：text/merge3/open + sections', () => {
  it('open 返回三方行数/块数/冲突数；三方行区间完备划分全表', async () => {
    const client = await newClient();
    const r = await openMerge3(client);
    expect(r.sessionId).toBe('m3-1');
    expect(r.chunkCount).toBe(10);
    expect(r.conflictCount).toBe(3); // 冲突含 1 个不重要自动采用
    expect(r.baseLineCount).toBe(10);
    expect(r.leftLineCount).toBe(11);
    expect(r.rightLineCount).toBe(11);
    expect(Number(r.rev)).toBe(0);

    const all = await sections(client, 'm3-1', 0, 100);
    expect(all.total).toBe(10);
    expect(all.hasMore).toBe(false);
    expect(all.chunks).toHaveLength(10);
    for (const side of ['base', 'left', 'right'] as const) {
      let cursor = 0;
      for (const c of all.chunks) {
        expect(c[side].start).toBe(cursor);
        expect(c[side].end).toBeGreaterThanOrEqual(c[side].start);
        cursor = c[side].end;
        expect((c[`${side}Lines`] ?? []).length).toBe(c[side].end - c[side].start);
      }
      expect(cursor).toBe(side === 'base' ? 10 : 11);
    }
    await client.close();
  });

  it('sections 覆盖五种来源与决策状态；分批窗口 + 越界钳制', async () => {
    const client = await newClient();
    await openMerge3(client);
    const origins = (await sections(client, 'm3-1', 0, 100)).chunks.map((c) => c.origin);
    expect(new Set(origins)).toEqual(
      new Set(['stable', 'leftOnly', 'rightOnly', 'bothChanged', 'conflict']),
    );

    const page = await sections(client, 'm3-1', 0, 4);
    expect(page.offset).toBe(0);
    expect(page.chunks.map((c) => c.id)).toEqual([0, 1, 2, 3]);
    expect(page.hasMore).toBe(true);
    const second = await sections(client, 'm3-1', 4, 100);
    expect(second.chunks.map((c) => c.id)).toEqual([4, 5, 6, 7, 8, 9]);
    expect(second.hasMore).toBe(false);
    expect((await sections(client, 'm3-1', 99, 3)).chunks).toHaveLength(0);

    const all = page.chunks.concat(second.chunks);
    const unresolved = all.filter((c) => c.unresolved).map((c) => c.id);
    expect(unresolved).toEqual([3, 7]);
    // 不重要冲突自动采用：不算未决，但仍是冲突块
    const auto = all.find((c) => c.id === 8)!;
    expect(auto.autoResolved).toBe(true);
    expect(auto.unresolved).toBe(false);
    expect(auto.conflictUnimportant).toBe(true);
    // 相同更改自动合并（bothChanged）
    const both = all.find((c) => c.origin === 'bothChanged')!;
    expect(both.outputLines).toEqual(both.leftLines);
    expect(both.unresolved).toBe(false);
    // addAdd：base 空区间占位 + 冲突行提示
    const addAdd = all.find((c) => c.conflictKind === 'addAdd')!;
    expect(addAdd.base.start).toBe(addAdd.base.end);
    expect(addAdd.conflictRows?.[0]?.unimportant).toBe(false);
    await client.close();
  });
});

describe('MockEngine P7：text/merge3/decide（幂等 + 六动作族）', () => {
  it('决策改变 rev/未决清单，重复提交幂等；输出随之变化', async () => {
    const client = await newClient();
    await openMerge3(client);
    const d: MergeDecideResult = await client.request('text/merge3/decide', {
      sessionId: 'm3-1',
      decisions: [{ chunkId: 3, action: 'left', text: null }],
    });
    expect(Number(d.rev)).toBe(1);
    expect(d.decided).toBe(1);
    expect(d.unresolvedChunks).toEqual([7]);

    const again: MergeDecideResult = await client.request('text/merge3/decide', {
      sessionId: 'm3-1',
      decisions: [{ chunkId: 3, action: 'left', text: null }],
    });
    expect(Number(again.rev)).toBe(1); // 幂等：rev 不变

    const c3 = (await sections(client, 'm3-1', 3, 1)).chunks[0];
    expect(c3.decided).toBe(true);
    expect(c3.decision).toBe('left');
    expect(c3.unresolved).toBe(false);
    expect(c3.outputLines).toEqual(['  const mode = "fast";']);
    const text = (await output(client, 'm3-1')).text;
    expect(text).toContain('  const mode = "fast";');
    expect(text).not.toContain('  const mode = "safe";');
    await client.close();
  });

  it('六动作族逐一改变 outputLines（含 manual 编辑文本）', async () => {
    const client = await newClient();
    await openMerge3(client);
    const cases: Array<{ action: string; text?: string; expect: string[] }> = [
      { action: 'left', expect: ['  const mode = "fast";'] },
      { action: 'right', expect: ['  const mode = "strict";'] },
      { action: 'base', expect: ['  const mode = "safe";'] },
      {
        action: 'leftThenRight',
        expect: ['  const mode = "fast";', '  const mode = "strict";'],
      },
      {
        action: 'rightThenLeft',
        expect: ['  const mode = "strict";', '  const mode = "fast";'],
      },
      {
        action: 'manual',
        text: '  const mode = "reviewed";\n',
        expect: ['  const mode = "reviewed";'],
      },
    ];
    for (const c of cases) {
      await client.request('text/merge3/decide', {
        sessionId: 'm3-1',
        decisions: [{ chunkId: 3, action: c.action, text: c.text ?? null }],
      });
      const chunk = (await sections(client, 'm3-1', 3, 1)).chunks[0];
      expect(chunk.outputLines).toEqual(c.expect);
      expect(chunk.decision).toBe(c.action);
    }
    await client.close();
  });

  it('批量提交清空未决冲突；非法块/manual 缺 text 整批拒绝', async () => {
    const client = await newClient();
    await openMerge3(client);
    const r: MergeDecideResult = await client.request('text/merge3/decide', {
      sessionId: 'm3-1',
      decisions: [
        { chunkId: 3, action: 'left', text: null },
        { chunkId: 7, action: 'right', text: null },
      ],
    });
    expect(r.decided).toBe(2);
    expect(r.unresolvedChunks).toEqual([]);
    const out = await output(client, 'm3-1');
    expect(out.hasConflicts).toBe(false);
    expect(out.decidedCount).toBe(2);

    await expect(
      client.request('text/merge3/decide', {
        sessionId: 'm3-1',
        decisions: [{ chunkId: 999, action: 'left', text: null }],
      }),
    ).rejects.toMatchObject({ code: -32602 });
    await expect(
      client.request('text/merge3/decide', {
        sessionId: 'm3-1',
        decisions: [{ chunkId: 3, action: 'manual', text: null }],
      }),
    ).rejects.toMatchObject({ code: -32602 });
    await client.close();
  });
});

describe('MockEngine P7：text/merge3/output（plain / marker）', () => {
  it('marker 模式为未决冲突渲染两处标记；diff3 风格含 base 段', async () => {
    const client = await newClient();
    await openMerge3(client);
    const plain = await output(client, 'm3-1');
    expect(plain.conflictCount).toBe(3);
    expect(plain.unresolvedChunks).toEqual([3, 7]);
    expect(plain.hasConflicts).toBe(true);
    expect(plain.text).not.toContain('<<<<<<<');

    const marker = await output(client, 'm3-1', 'marker');
    expect(marker.text.match(/<<<<<<</g)).toHaveLength(2);
    expect(marker.text).toContain('/demo/left.txt');
    expect(marker.text).not.toContain('|||||||');

    const diff3 = await output(client, 'm3-1', 'marker', 'diff3');
    expect(diff3.text.match(/\|\|\|\|\|\|\|/g)).toHaveLength(2);
    expect(diff3.text).toContain('/demo/base.txt');

    // 全部决策后 marker 不再有标记
    await client.request('text/merge3/decide', {
      sessionId: 'm3-1',
      decisions: [
        { chunkId: 3, action: 'left', text: null },
        { chunkId: 7, action: 'left', text: null },
      ],
    });
    expect((await output(client, 'm3-1', 'marker')).text).not.toContain('<<<<<<<');
    await client.close();
  });
});

describe('MockEngine P7：错误路径与会话清理', () => {
  it('会话 411 → session/close → 重新 open（StrictMode 幂等重建基础）', async () => {
    const client = await newClient();
    const p = {
      sessionId: 'm3-idem',
      base: '/demo/base.txt',
      left: '/demo/left.txt',
      right: '/demo/right.txt',
      options: null,
    };
    await client.request('text/merge3/open', p);
    await expect(client.request('text/merge3/open', p)).rejects.toMatchObject({ code: 411 });
    expect(await client.request('session/close', { sessionId: 'm3-idem' })).toBeNull();
    const again = await client.request<MergeOpenResult>('text/merge3/open', p);
    expect(again.chunkCount).toBeGreaterThan(0);
    await client.close();
  });
  it('会话不存在报 410；重复 open 报 411；close 幂等', async () => {
    const client = await newClient();
    await expect(sections(client, 'ghost', 0, 5)).rejects.toMatchObject({ code: 410 });
    await expect(output(client, 'ghost', null, null)).rejects.toMatchObject({ code: 410 });
    await expect(
      client.request('text/merge3/decide', { sessionId: 'ghost', decisions: [] }),
    ).rejects.toMatchObject({ code: 410 });

    await openMerge3(client);
    await expect(openMerge3(client)).rejects.toMatchObject({ code: 411 });
    expect(await client.request('session/close', { sessionId: 'm3-1' })).toBeNull();
    expect(await client.request('session/close', { sessionId: 'm3-1' })).toBeNull();
    await expect(sections(client, 'm3-1', 0, 5)).rejects.toMatchObject({ code: 410 });
    await client.close();
  });

  it('非合并会话句柄被拒绝；空 sessionId 报 -32602', async () => {
    const client = await newClient();
    await client.request('text/open', {
      sessionId: 'text-x',
      left: '/a/x.c',
      right: '/b/x.c',
      options: null,
    });
    await expect(sections(client, 'text-x', 0, 5)).rejects.toMatchObject({ code: -32602 });
    await expect(
      client.request('text/merge3/open', {
        sessionId: '',
        base: '/b',
        left: '/l',
        right: '/r',
        options: null,
      }),
    ).rejects.toMatchObject({ code: -32602 });
    await client.close();
  });
});

describe('MockEngine P7：text/merge3/save（R3-20，FR-TMRG-006）', () => {
  const save = (client: RpcClient, sessionId: string, extra: Record<string, unknown> = {}) =>
    client.request<MergeSaveResult>('text/merge3/save', { sessionId, ...extra });

  it('initialize 宣告 mergeSave 能力位', async () => {
    const client = await newClient();
    const init = await client.request<{ capabilities: { mergeSave?: boolean } }>('initialize', {
      clientInfo: { name: 'test', version: '0' },
      protocolVersion: 1,
    });
    expect(init.capabilities.mergeSave).toBe(true);
    await client.close();
  });

  it('写入合成输出：字节/行数与 output 一致，新目标不备份，rev 不递增', async () => {
    const client = await newClient();
    await openMerge3(client);
    await client.request('text/merge3/decide', {
      sessionId: 'm3-1',
      decisions: [
        { chunkId: 3, action: 'left', text: null },
        { chunkId: 7, action: 'right', text: null },
      ],
    });
    const out = await output(client, 'm3-1');
    const res = await save(client, 'm3-1', { path: '/tmp/aic-m3-out.txt' });
    expect(res.path).toBe('/tmp/aic-m3-out.txt');
    expect(Number(res.bytes)).toBe(out.text.length);
    expect(res.lineCount).toBe(out.text.split('\n').length - 1);
    expect(res.backupPath).toBeUndefined();
    expect(Number(res.rev)).toBe(1); // 决策后 rev=1，保存不递增
    expect(res.hasConflicts).toBe(false);
    expect(res.unresolvedChunks).toEqual([]);
    expect(res.conflictCount).toBe(3);
    await client.close();
  });

  it('二次保存产生 .bak（证明首次写入已落地）；backup=false 不备份', async () => {
    const client = await newClient();
    await openMerge3(client);
    const path = '/tmp/aic-m3-again.txt';
    const first = await save(client, 'm3-1', { path });
    expect(first.backupPath).toBeUndefined();
    const second = await save(client, 'm3-1', { path });
    expect(second.backupPath).toBe(`${path}.bak`);
    const third = await save(client, 'm3-1', { path, backup: false });
    expect(third.backupPath).toBeUndefined();
    await client.close();
  });

  it('marker 模式保存未决冲突：回报冲突清单，字节数与 marker 输出一致', async () => {
    const client = await newClient();
    await openMerge3(client);
    const marker = await output(client, 'm3-1', 'marker');
    const res = await save(client, 'm3-1', {
      path: '/tmp/aic-m3-marker.txt',
      mode: 'marker',
      backup: false,
    });
    expect(Number(res.bytes)).toBe(marker.text.length);
    expect(res.hasConflicts).toBe(true);
    expect(res.unresolvedChunks).toEqual([3, 7]);
    expect(res.backupPath).toBeUndefined();
    await client.close();
  });

  it('manual 决策文本进入落盘输出（行数随编辑变化）', async () => {
    const client = await newClient();
    await openMerge3(client);
    const before = await output(client, 'm3-1');
    await client.request('text/merge3/decide', {
      sessionId: 'm3-1',
      decisions: [{ chunkId: 3, action: 'manual', text: 'L1\nL2\nL3\n' }],
    });
    const after = await output(client, 'm3-1');
    expect(after.text).not.toBe(before.text);
    const res = await save(client, 'm3-1', { path: '/tmp/aic-m3-manual.txt' });
    expect(Number(res.bytes)).toBe(after.text.length);
    expect(res.lineCount).toBe(after.text.split('\n').length - 1);
    await client.close();
  });

  it('空路径 / 未知会话 / 非合并会话被拒', async () => {
    const client = await newClient();
    await openMerge3(client);
    await expect(save(client, 'm3-1', { path: '' })).rejects.toMatchObject({ code: -32602 });
    await expect(save(client, 'ghost', { path: '/tmp/x' })).rejects.toMatchObject({ code: 410 });
    await client.request('text/open', {
      sessionId: 'm3-text',
      left: '/a/x.c',
      right: '/b/x.c',
      options: null,
    });
    await expect(save(client, 'm3-text', { path: '/tmp/x' })).rejects.toMatchObject({
      code: -32602,
    });
    await client.close();
  });
});
