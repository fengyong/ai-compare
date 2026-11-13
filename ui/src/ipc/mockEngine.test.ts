import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type { DirDelta } from '../protocol/types';

/**
 * MockEngine 与真实 ai-engine P0 方法集对齐后的协议面集成测试：
 * initialize / dir/open（客户端分配 sessionId）→ dir/delta 批次通知 /
 * text/open → text/lines 分页 / session/close 幂等。
 *
 * 注：子会话关闭 → 父目录行 fileOverride 回传属引擎 P1+（真实引擎 P0
 * 亦未开放该 RPC），原「回传链路」测试随协议面收窄一并移除。
 */

const newClient = async () => {
  const engine = new MockEngine();
  const client = new RpcClient(new MockTransport(engine));
  await client.ready;
  return client;
};

describe('MockEngine P0 协议面', () => {
  it('dir/open 应答先行，dir/delta 分批推送且 rev 单调递增、批次收尾 hasMore=false', async () => {
    const client = await newClient();
    const deltas: DirDelta[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDelta));

    const opened = await client.request<{ sessionId: string; rowCount: number; scanState: string }>(
      'dir/open',
      { sessionId: 'dir-1', left: '/a', right: '/b', depth: null, criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' } }
    );
    expect(opened.sessionId).toBe('dir-1');
    expect(opened.scanState).toBe('completed');
    expect(opened.rowCount).toBeGreaterThan(0);

    // 后台泵推完剩余批次
    await new Promise((r) => setTimeout(r, 300));

    expect(deltas.length).toBeGreaterThan(0);
    expect(deltas.every((d) => d.sessionId === 'dir-1')).toBe(true);
    const revs = deltas.map((d) => Number(d.rev));
    expect([...revs].sort((a, b) => a - b)).toEqual(revs); // 单调递增
    expect(deltas[deltas.length - 1].batch.hasMore).toBe(false);

    // 行结构与 DirRow 生成类型一致；upsert 按行 id 合并后总数 == rowCount
    const ids = new Set<string>();
    for (const d of deltas) for (const u of d.upserts) ids.add(u.id);
    expect(ids.size).toBe(opened.rowCount);
    const first = deltas[0].upserts[0];
    expect(typeof first.id).toBe('string');
    expect(['file', 'dir']).toContain(first.kind);
    await client.close();
  });

  it('text/lines 分页返回行内容且 done 正确', async () => {
    const client = await newClient();
    const text = await client.request<{ sessionId: string; leftLineCount: number }>('text/open', {
      sessionId: 'text-1',
      left: '/a/src/main.c',
      right: '/b/src/main.c',
      options: null,
    });
    expect(text.leftLineCount).toBeGreaterThan(0);

    const all: string[] = [];
    let start = 0;
    let done = false;
    while (!done) {
      const res = await client.request<{ start: number; lines: string[]; done: boolean }>('text/lines', {
        sessionId: text.sessionId,
        side: 'left',
        start,
        count: 3, // 小页：必然多页，验证 done 语义
      });
      expect(res.start).toBe(start);
      all.push(...res.lines);
      done = res.done;
      start += res.lines.length;
    }
    expect(all.length).toBe(text.leftLineCount);
    await client.close();
  });

  it('session/close 幂等；关闭后访问会话报 SESSION_NOT_FOUND(410)', async () => {
    const client = await newClient();
    await client.request('text/open', {
      sessionId: 'text-2',
      left: '/a/x.c',
      right: '/b/x.c',
      options: null,
    });
    expect(await client.request('session/close', { sessionId: 'text-2' })).toBeNull();
    // 幂等：不存在也成功
    expect(await client.request('session/close', { sessionId: 'text-2' })).toBeNull();

    await expect(
      client.request('text/sections', { sessionId: 'text-2', offset: 0, count: 10 })
    ).rejects.toMatchObject({ code: 410 });
    await client.close();
  });
});
