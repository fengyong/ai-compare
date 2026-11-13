import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type { DirDeltaParams } from '../protocol/types';

/** 集成级：目录会话 → 子会话打开/关闭 → dir/delta 回传 fileOverride（docs/03 §5.1 最小闭环） */
describe('MockEngine 子会话回传链路', () => {
  it('closeText(reportStatus) 后父目录会话收到该行的 fileOverride upsert', async () => {
    const engine = new MockEngine();
    const transport = new MockTransport(engine);
    const client = new RpcClient(transport);
    await client.ready;

    const deltas: DirDeltaParams[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDeltaParams));

    const opened = await client.request<{ sessionId: string; rowCount: number }>('dir/open', {
      left: '/a',
      right: '/b',
    });
    const dirId = opened.sessionId;

    // 等"后台扫描"泵把剩余 delta 推完
    await new Promise((r) => setTimeout(r, 300));

    // 找一个文件行
    const rowsRes = await client.request<{ rows: Array<{ rowId: number; name: string; kind: string }> }>(
      'dir/rows',
      { sessionId: dirId, offset: 0, count: 500 }
    );
    const fileRow = rowsRes.rows.find((r) => r.kind === 'file');
    expect(fileRow).toBeDefined();

    const child = await client.request<{ kind: string; viewKind: string; left: unknown; right: unknown }>(
      'dir/openChild',
      { sessionId: dirId, rowId: fileRow!.rowId }
    );
    expect(child.viewKind).toBe('text');

    const textOpened = await client.request<{ sessionId: string }>('text/open', {
      left: child.left,
      right: child.right,
      parent: { sessionId: dirId, rowId: fileRow!.rowId },
    });

    const closeRes = await client.request<{ reportedStatus: string | null }>('text/close', {
      sessionId: textOpened.sessionId,
      reportStatus: true,
    });
    expect(closeRes.reportedStatus).toBe('contentDiff');

    // 回传 delta：包含该行的 fileOverride
    const overrideDelta = deltas.find(
      (d) =>
        d.sessionId === dirId &&
        d.upserts.some((u) => u.row.rowId === fileRow!.rowId && u.row.fileOverride === 'contentDiff')
    );
    expect(overrideDelta).toBeDefined();

    await client.close();
  });

  it('text/lines 分页返回行内容且 done 正确', async () => {
    const engine = new MockEngine();
    const client = new RpcClient(new MockTransport(engine));
    await client.ready;
    const text = await client.request<{ sessionId: string; left: { lineCount: number } }>('text/open', {
      left: { name: 'a.c' },
      right: { name: 'a.c' },
    });
    const all: string[] = [];
    let start = 0;
    let done = false;
    while (!done) {
      const res = await client.request<{ lines: string[]; done: boolean }>('text/lines', {
        sessionId: text.sessionId,
        side: 'left',
        start,
        count: 100,
      });
      all.push(...res.lines);
      done = res.done;
      start += res.lines.length;
      if (start > text.left.lineCount + 100) break;
    }
    expect(all.length).toBe(text.left.lineCount);
    await client.close();
  });
});
