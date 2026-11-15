import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import { clipboardStore } from '../lib/clipboardStore';
import type {
  TextEditResult,
  TextLinesResult,
  TextOpenResult,
  TextSaveResult,
} from '../protocol/types';

/**
 * MockEngine text/edit（R3-18 编辑增量重 diff）与真实引擎 handlers.rs::text_edit
 * 同形：批次原子应用、顺序坐标、end 省略 = 整侧重放、rev 单调递增、
 * 应答携带引擎权威 sections/summary。
 */

const newClient = async () => {
  const engine = new MockEngine();
  const client = new RpcClient(new MockTransport(engine));
  await client.ready;
  return client;
};

async function openPair(
  client: RpcClient,
  sessionId: string,
  left: string,
  right: string,
): Promise<TextOpenResult> {
  return client.request<TextOpenResult>('text/open', {
    sessionId,
    left: clipboardStore.registerText(left),
    right: clipboardStore.registerText(right),
    options: null,
  });
}

async function sideLines(
  client: RpcClient,
  sessionId: string,
  side: 'left' | 'right',
): Promise<string[]> {
  const r = await client.request<TextLinesResult>('text/lines', {
    sessionId,
    side,
    start: 0,
    count: 10_000,
  });
  return r.lines;
}

const edit = (client: RpcClient, sessionId: string, edits: unknown[]) =>
  client.request<TextEditResult>('text/edit', { sessionId, edits });

describe('MockEngine text/open 单文件会话（D2）', () => {
  it('省略 right：只建左表、不跑 diff、rightEncoding 为空', async () => {
    const client = await newClient();
    const opened = await client.request<TextOpenResult>('text/open', {
      sessionId: 'one-1',
      left: clipboardStore.registerText('alpha\nbeta\n'),
    });
    expect(opened.leftLineCount).toBe(2);
    expect(opened.rightLineCount).toBe(0);
    expect(opened.sectionCount).toBe(0);
    expect(opened.rightEncoding).toBe('');
    const lines = await client.request<TextLinesResult>('text/lines', {
      sessionId: 'one-1',
      side: 'left',
      start: 0,
      count: 10,
    });
    expect(lines.lines).toEqual(['alpha', 'beta']);
    // 右表为空 → 空页而非报错（与真实引擎同形）
    const right = await client.request<TextLinesResult>('text/lines', {
      sessionId: 'one-1',
      side: 'right',
      start: 0,
      count: 10,
    });
    expect(right.lines).toEqual([]);
    expect(right.done).toBe(true);
  });

  it('单文件会话可编辑并落盘（text/edit + text/save side=left）', async () => {
    const client = await newClient();
    await client.request<TextOpenResult>('text/open', {
      sessionId: 'one-2',
      left: clipboardStore.registerText('alpha\nbeta'),
    });
    const edited = await client.request<TextEditResult>('text/edit', {
      sessionId: 'one-2',
      edits: [{ side: 'left', start: 1, end: 2, text: 'BETA' }],
    });
    expect(edited.leftLineCount).toBe(2);
    const saved = await client.request<TextSaveResult>('text/save', {
      sessionId: 'one-2',
      side: 'left',
      backup: false,
    });
    expect(saved.saved).toHaveLength(1);
    expect(saved.leftModified).toBe(false);
  });
});

describe('MockEngine text/edit：编辑增量重 diff（R3-18）', () => {
  it('initialize 宣告 textEdit 能力位', async () => {
    const client = await newClient();
    const init = await client.request<{ capabilities: { textEdit?: boolean } }>('initialize', {
      clientInfo: { name: 'test', version: '0' },
      protocolVersion: 1,
    });
    expect(init.capabilities.textEdit).toBe(true);
    await client.close();
  });

  it('应用编辑后行内容、sections、summary、rev 同步更新', async () => {
    const client = await newClient();
    const opened = await openPair(client, 'te-1', 'alpha\nshared\n', 'beta\nshared\n');
    expect(opened.summary.changeSections).toBeGreaterThan(0);

    const res = await edit(client, 'te-1', [{ side: 'left', start: 0, end: 1, text: 'beta' }]);
    expect(res.sessionId).toBe('te-1');
    expect(Number(res.rev)).toBe(1);
    expect(res.leftLineCount).toBe(2);
    expect(res.rightLineCount).toBe(2);
    expect(await sideLines(client, 'te-1', 'left')).toEqual(['beta', 'shared']);
    // 两侧已完全一致 → 引擎权威 summary 归零（客户端重 diff 亦应如此，
    // 但重要性分级只有引擎口径能给，见下条）
    expect(res.summary.changeSections).toBe(0);
    expect(res.sectionCount).toBe(res.sections.length);
    expect(res.sections.every((s) => s.kind === 'equal')).toBe(true);
    await client.close();
  });

  it('批次原子性：任一 op 越界则整批不落地，rev 不变', async () => {
    const client = await newClient();
    await openPair(client, 'te-2', 'a\nb\nc\n', 'a\nb\nc\n');
    const before = await sideLines(client, 'te-2', 'left');

    await expect(
      edit(client, 'te-2', [
        { side: 'left', start: 0, end: 1, text: 'X' },
        { side: 'left', start: 9, end: 10, text: 'Y' },
      ]),
    ).rejects.toMatchObject({ code: -32602 });

    expect(await sideLines(client, 'te-2', 'left')).toEqual(before);
    const ok = await edit(client, 'te-2', [{ side: 'left', start: 0, end: 1, text: 'Z' }]);
    expect(Number(ok.rev)).toBe(1); // 失败批次未消耗 rev
    await client.close();
  });

  it('顺序 op 使用应用后坐标（同批多 op 不串位）', async () => {
    const client = await newClient();
    await openPair(client, 'te-3', 'a\nb\nc\n', 'a\nb\nc\n');
    const res = await edit(client, 'te-3', [
      { side: 'left', start: 0, end: 0, text: 'head' }, // 插入 → a b c 前移
      { side: 'left', start: 2, end: 3, text: 'B' }, // 此时 index2 == 原 'b'
    ]);
    expect(res.leftLineCount).toBe(4);
    expect(await sideLines(client, 'te-3', 'left')).toEqual(['head', 'a', 'B', 'c']);
    await client.close();
  });

  it('end 省略 = 整侧重放（分叉收敛手段）', async () => {
    const client = await newClient();
    await openPair(client, 'te-4', 'a\nb\nc\n', 'x\n');
    const res = await edit(client, 'te-4', [{ side: 'left', start: 0, text: 'p\nq' }]);
    expect(res.leftLineCount).toBe(2);
    expect(await sideLines(client, 'te-4', 'left')).toEqual(['p', 'q']);
    await client.close();
  });

  it('空 text = 纯删除；空侧可重放为空', async () => {
    const client = await newClient();
    await openPair(client, 'te-5', 'a\nb\nc\n', 'z\n');
    await edit(client, 'te-5', [{ side: 'left', start: 1, end: 2, text: '' }]);
    expect(await sideLines(client, 'te-5', 'left')).toEqual(['a', 'c']);
    const res = await edit(client, 'te-5', [{ side: 'left', start: 0, text: '' }]);
    expect(res.leftLineCount).toBe(0);
    expect(await sideLines(client, 'te-5', 'left')).toEqual([]);
    await client.close();
  });

  it('空批次 / 非法 side / 未知会话被拒', async () => {
    const client = await newClient();
    await openPair(client, 'te-6', 'a\n', 'a\n');
    await expect(edit(client, 'te-6', [])).rejects.toMatchObject({ code: -32602 });
    await expect(
      edit(client, 'te-6', [{ side: 'middle', start: 0, end: 1, text: 'x' }]),
    ).rejects.toMatchObject({ code: -32602 });
    await expect(
      edit(client, 'ghost', [{ side: 'left', start: 0, end: 1, text: 'x' }]),
    ).rejects.toMatchObject({ code: 410 });
    await client.close();
  });

  it('目录会话不接受 text/edit（会话类型隔离）', async () => {
    const client = await newClient();
    await client.request('dir/open', {
      sessionId: 'te-dir',
      left: '/a',
      right: '/b',
      depth: null,
      criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
    });
    await expect(
      edit(client, 'te-dir', [{ side: 'left', start: 0, end: 1, text: 'x' }]),
    ).rejects.toMatchObject({ code: -32602 });
    await client.close();
  });
});

/**
 * MockEngine text/save（R3-04 编辑内容落盘）与引擎 handlers.rs::text_save 同形：
 * both 只写编辑过的侧、显式侧无条件写、备份缺省开启、另存为改指目标、
 * 保存不递增 rev、落盘后清除该侧未保存标记。
 */
describe('MockEngine text/save：编辑内容落盘（R3-04）', () => {
  const save = (client: RpcClient, sessionId: string, extra: Record<string, unknown> = {}) =>
    client.request<TextSaveResult>('text/save', { sessionId, ...extra });

  it('initialize 宣告 textSave 能力位', async () => {
    const client = await newClient();
    const init = await client.request<{ capabilities: { textSave?: boolean } }>('initialize', {
      clientInfo: { name: 'test', version: '0' },
      protocolVersion: 1,
    });
    expect(init.capabilities.textSave).toBe(true);
    await client.close();
  });

  it('both 只写编辑过的侧：rev 不递增、脏标记清除', async () => {
    const client = await newClient();
    await openPair(client, 'ts-1', 'a\nb\n', 'a\nB\n');
    const edited = await edit(client, 'ts-1', [{ side: 'left', start: 1, end: 2, text: 'BB' }]);
    const res = await save(client, 'ts-1');
    expect(res.saved.map((e) => e.side)).toEqual(['left']);
    expect(res.saved[0].lineCount).toBe(2);
    expect(res.saved[0].bytes).toBe(5);
    expect(res.rev).toBe(edited.rev);
    expect(res.leftModified).toBe(false);
    expect(res.rightModified).toBe(false);
    await client.close();
  });

  it('无编辑时保存是空操作', async () => {
    const client = await newClient();
    await openPair(client, 'ts-2', 'a\n', 'b\n');
    const res = await save(client, 'ts-2');
    expect(res.saved).toEqual([]);
    expect(res.rev).toBe(0);
    await client.close();
  });

  it('备份缺省开启；backup=false 不产生 backupPath', async () => {
    const client = await newClient();
    const opened = await client.request<TextOpenResult>('text/open', {
      sessionId: 'ts-3',
      left: '/tmp/aic-ts3-left.txt',
      right: '/tmp/aic-ts3-right.txt',
      options: null,
    });
    expect(opened.leftLineCount).toBeGreaterThan(0);
    await edit(client, 'ts-3', [{ side: 'left', start: 0, end: 1, text: 'X' }]);
    const withBak = await save(client, 'ts-3');
    expect(withBak.saved[0].backupPath).toBe('/tmp/aic-ts3-left.txt.bak');
    await edit(client, 'ts-3', [{ side: 'left', start: 0, end: 1, text: 'Y' }]);
    const noBak = await save(client, 'ts-3', { backup: false });
    expect(noBak.saved[0].backupPath).toBeUndefined();
    await client.close();
  });

  it('显式侧无条件写；另存为改指目标且新目标不备份', async () => {
    const client = await newClient();
    await client.request('text/open', {
      sessionId: 'ts-4',
      left: '/tmp/aic-ts4-left.txt',
      right: '/tmp/aic-ts4-right.txt',
      options: null,
    });
    const explicit = await save(client, 'ts-4', { side: 'right' });
    expect(explicit.saved.map((e) => e.side)).toEqual(['right']);
    expect(explicit.saved[0].backupPath).toBe('/tmp/aic-ts4-right.txt.bak');

    await edit(client, 'ts-4', [{ side: 'left', start: 0, end: 1, text: 'Z' }]);
    const as = await save(client, 'ts-4', { side: 'left', leftPath: '/tmp/aic-ts4-new.txt' });
    expect(as.saved[0].path).toBe('/tmp/aic-ts4-new.txt');
    expect(as.saved[0].backupPath).toBeUndefined();
    // 再次编辑后缺省保存应写新目标（原文件不再被覆盖）
    await edit(client, 'ts-4', [{ side: 'left', start: 0, end: 1, text: 'W' }]);
    const again = await save(client, 'ts-4');
    expect(again.saved[0].path).toBe('/tmp/aic-ts4-new.txt');
    await client.close();
  });

  it('未知会话 / 目录会话被拒', async () => {
    const client = await newClient();
    await expect(save(client, 'ghost')).rejects.toMatchObject({ code: 410 });
    await client.request('dir/open', {
      sessionId: 'ts-dir',
      left: '/a',
      right: '/b',
      depth: null,
      criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
    });
    await expect(save(client, 'ts-dir')).rejects.toMatchObject({ code: -32602 });
    await client.close();
  });
});
