import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import { clipboardStore } from '../lib/clipboardStore';
import type { TextEditResult, TextLinesResult, TextOpenResult } from '../protocol/types';

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
