import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type { DirDelta } from '../protocol/types';
import { clipboardStore } from '../lib/clipboardStore';

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
      {
        sessionId: 'dir-1',
        left: '/a',
        right: '/b',
        depth: null,
        criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
      },
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
      const res = await client.request<{ start: number; lines: string[]; done: boolean }>(
        'text/lines',
        {
          sessionId: text.sessionId,
          side: 'left',
          start,
          count: 3, // 小页：必然多页，验证 done 语义
        },
      );
      expect(res.start).toBe(start);
      all.push(...res.lines);
      done = res.done;
      start += res.lines.length;
    }
    expect(all.length).toBe(text.leftLineCount);
    await client.close();
  });

  it('text/inlineDiff（P3 生成协议形）：按 section 懒计算，绝对行号 + 成对区间 + truncated', async () => {
    const client = await newClient();
    const opened = await client.request<{ sessionId: string; sectionCount: number }>('text/open', {
      sessionId: 'text-inline',
      left: '/a/main.c',
      right: '/b/main.c',
      options: null,
    });
    // 找一个 change section
    const secs = await client.request<{
      sections: Array<{
        id: number;
        kind: string;
        left: { start: number; end: number } | null;
        right: { start: number; end: number } | null;
      }>;
    }>('text/sections', { sessionId: opened.sessionId, offset: 0, count: 10_000 });
    const change = secs.sections.find((s) => s.kind === 'change' && s.left && s.right);
    expect(change).toBeTruthy();
    const res = await client.request<{
      sessionId: string;
      sectionId: number;
      kind: string;
      state: string;
      lines: Array<{
        leftLine: number;
        rightLine: number;
        spans: Array<{ left: unknown; right: unknown; kind: string }>;
        truncated: boolean;
      }>;
    }>('text/inlineDiff', { sessionId: opened.sessionId, sectionId: change!.id });
    expect(res.sessionId).toBe(opened.sessionId);
    expect(res.sectionId).toBe(change!.id);
    expect(res.kind).toBe('change');
    expect(['full', 'partial']).toContain(res.state);
    expect(res.lines.length).toBeGreaterThan(0);
    // 绝对行号落在 section 区间内
    const first = res.lines[0];
    expect(first.leftLine).toBeGreaterThanOrEqual(change!.left!.start);
    expect(first.leftLine).toBeLessThan(change!.left!.end);
    expect(first.rightLine).toBeGreaterThanOrEqual(change!.right!.start);
    expect(first.rightLine).toBeLessThan(change!.right!.end);
    // 非 equal span 成对（replace 双侧 / insert 仅右 / delete 仅左）
    for (const span of first.spans) {
      expect(['replace', 'insert', 'delete']).toContain(span.kind);
      if (span.kind === 'replace') {
        expect(span.left).toBeTruthy();
        expect(span.right).toBeTruthy();
      } else if (span.kind === 'insert') {
        expect(span.left).toBeNull();
        expect(span.right).toBeTruthy();
      } else {
        expect(span.left).toBeTruthy();
        expect(span.right).toBeNull();
      }
    }
    // 未知 section → INVALID_PARAMS
    await expect(
      client.request('text/inlineDiff', { sessionId: opened.sessionId, sectionId: 999_999 }),
    ).rejects.toMatchObject({ code: -32602 });
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
      client.request('text/sections', { sessionId: 'text-2', offset: 0, count: 10 }),
    ).rejects.toMatchObject({ code: 410 });
    await client.close();
  });

  it('picture/open 返回图片形态（P14 dispatch 落穿回归锚定）', async () => {
    const client = await newClient();
    const pic = await client.request<{ width: number; blocks: unknown[]; similarityPct: number }>(
      'picture/open',
      { sessionId: 'pic-1', left: '/demo/a.png', right: '/demo/b.png', options: null },
    );
    expect(pic.width).toBeGreaterThan(0);
    expect(Array.isArray(pic.blocks)).toBe(true);
    expect(pic.similarityPct).toBeGreaterThan(0);
    // 同一 dispatch 内 hex/open 仍返回 hex 形态，两者不得互相落穿
    const hex = await client.request<{ leftSize: number; blockCount: number }>('hex/open', {
      sessionId: 'hex-open-1',
      left: '/demo/a.bin',
      right: '/demo/b.bin',
      options: null,
    });
    expect(hex.leftSize).toBeGreaterThan(0);
    expect(hex.blockCount).toBeGreaterThan(0);
    await client.close();
  });

  it('report/generate 同形：布局族 / 选项回显 / 分块流 / 未知会话', async () => {
    const client = await newClient();
    await client.request('text/open', {
      sessionId: 'rpt-1',
      left: '/demo/a.txt',
      right: '/demo/b.txt',
      options: null,
    });
    const full = await client.request<{ content: string; totalChars: number; hasMore: boolean }>(
      'report/generate',
      { sessionId: 'rpt-1', layout: 'summary', title: 'demo title', lineNumbers: true },
    );
    expect(full.content).toContain('demo title');
    expect(full.content).toContain('lineNumbers=true');
    expect(full.hasMore).toBe(false);
    expect(full.totalChars).toBe(Array.from(full.content).length);

    const html = await client.request<{ content: string }>('report/generate', {
      sessionId: 'rpt-1',
      layout: 'html',
      color: 'mono',
    });
    expect(html.content).toContain('<!doctype html>');
    expect(html.content).toContain('color=mono');

    const first = await client.request<{
      content: string;
      offset: number;
      hasMore: boolean;
      totalChars: number;
    }>('report/generate', { sessionId: 'rpt-1', layout: 'xml', offset: 0, limit: 10 });
    expect(first.hasMore).toBe(true);
    expect(Array.from(first.content).length).toBe(10);
    let assembled = first.content;
    let off = 10;
    while (off < first.totalChars) {
      const blk = await client.request<{ content: string; offset: number; hasMore: boolean }>(
        'report/generate',
        { sessionId: 'rpt-1', layout: 'xml', offset: off, limit: 7 },
      );
      assembled += blk.content;
      off += Array.from(blk.content).length;
      if (!blk.hasMore || blk.content.length === 0) break;
    }
    const whole = await client.request<{ content: string }>('report/generate', {
      sessionId: 'rpt-1',
      layout: 'xml',
      offset: 0,
      limit: 10000,
    });
    expect(assembled).toBe(whole.content);
    await expect(
      client.request('report/generate', { sessionId: 'ghost', layout: 'summary' }),
    ).rejects.toMatchObject({ code: 410 });
    await client.close();
  });

  it('text/open 的 clipboard:// 规格取剪贴板历史文本（FR-CLIP-002 mock 同形）', async () => {
    const client = await newClient();
    const leftSpec = clipboardStore.registerText('alpha\nshared\n');
    const rightSpec = clipboardStore.registerText('beta\nshared\n');
    const r = await client.request<{ leftLineCount: number; rightLineCount: number }>('text/open', {
      sessionId: 'clip-mock-1',
      left: leftSpec,
      right: rightSpec,
      options: null,
    });
    expect(r.leftLineCount).toBe(2);
    expect(r.rightLineCount).toBe(2);
    const left = await client.request<{ lines: string[] }>('text/lines', {
      sessionId: 'clip-mock-1',
      side: 'left',
      start: 0,
      count: 10,
    });
    const right = await client.request<{ lines: string[] }>('text/lines', {
      sessionId: 'clip-mock-1',
      side: 'right',
      start: 0,
      count: 10,
    });
    expect(left.lines).toEqual(['alpha', 'shared']);
    expect(right.lines).toEqual(['beta', 'shared']);
    await client.close();
  });
});

describe('MockEngine 编码参数（R3-15：选择器必须真的上协议面）', () => {
  const openWith = async (encoding?: string) => {
    const client = await newClient();
    try {
      return await client.request<{ leftEncoding: string; rightEncoding: string }>('text/open', {
        sessionId: 'enc-mock-1',
        left: '/a/x.txt',
        right: '/b/x.txt',
        options: null,
        ...(encoding === undefined ? {} : { encoding }),
      });
    } finally {
      await client.close();
    }
  };

  it('缺省/auto：回传 utf-8（mock 内容为合成文本）', async () => {
    expect((await openWith()).leftEncoding).toBe('utf-8');
    expect((await openWith('auto')).leftEncoding).toBe('utf-8');
  });

  it('显式编码：两侧回传同一生效标签', async () => {
    const r = await openWith('gb18030');
    expect(r.leftEncoding).toBe('gb18030');
    expect(r.rightEncoding).toBe('gb18030');
    expect((await openWith('utf-16le')).leftEncoding).toBe('utf-16le');
  });

  it('未知标签 → 413，且不留下半开会话（同句柄可重试）', async () => {
    const client = await newClient();
    const args = { sessionId: 'enc-mock-2', left: '/a/x.txt', right: '/b/x.txt', options: null };
    let code: unknown = null;
    try {
      await client.request('text/open', { ...args, encoding: 'no-such-encoding' });
    } catch (e) {
      code = (e as { code?: number }).code;
    }
    expect(code).toBe(413);
    // 失败不得占用会话表：否则用户改选编码后重试会撞 411
    const retry = await client.request<{ leftEncoding: string }>('text/open', {
      ...args,
      encoding: 'big5',
    });
    expect(retry.leftEncoding).toBe('big5');
    await client.close();
  });
});
