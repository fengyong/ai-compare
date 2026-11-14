import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type {
  ContentCompareMethod,
  DirCompareContentResult,
  DirDelta,
  DirRow,
  InitializeResult,
} from '../protocol/types';

/**
 * R3-16：dir/compareContent 的演示引擎同形面（FR-DIR-011 三方式 /
 * FR-DIR-012 手动比较五态结果）。真实引擎口径见 ai-dir::verdict 与
 * engine/ai-engine handlers 的 5 条 p2_tests；此处只保证 UI 演示不漂移。
 */

const newClient = async () => {
  const engine = new MockEngine();
  const client = new RpcClient(new MockTransport(engine));
  await client.ready;
  return client;
};

async function openSession(method?: ContentCompareMethod) {
  const client = await newClient();
  const deltas: DirDelta[] = [];
  client.on('dir/delta', (p) => deltas.push(p as DirDelta));
  await client.request('dir/open', {
    sessionId: 'cc',
    left: '/demo/l',
    right: '/demo/r',
    depth: null,
    criteria: {
      mtimeToleranceSec: 2,
      contentCompare: 'auto',
      ...(method ? { contentMethod: method } : {}),
    },
  });
  const rows: DirRow[] = [];
  for (const d of deltas) rows.push(...d.upserts);
  return { client, rows };
}

const pairedFiles = (rows: DirRow[]) =>
  rows.filter((r) => r.kind === 'file' && r.left !== null && r.right !== null);

const compare = (client: RpcClient, ids: string[], method?: ContentCompareMethod) =>
  client.request<DirCompareContentResult>('dir/compareContent', {
    sessionId: 'cc',
    ids,
    ...(method ? { method } : {}),
  });

describe('MockEngine R3-16：能力协商', () => {
  it('initialize 声明 dirCompareContent=true（新能力位显式开启）', async () => {
    const client = await newClient();
    const init = (await client.ready) as InitializeResult;
    expect(init.capabilities.dirCompareContent).toBe(true);
    await client.close();
  });
});

describe('MockEngine R3-16：dir/compareContent', () => {
  it('ids 为空 = 比较全部已配对文件行（目录行与孤立项不参与）', async () => {
    const { client, rows } = await openSession();
    const res = await compare(client, []);
    const targets = pairedFiles(rows);
    expect(res.results.map((r) => r.id).sort()).toEqual(targets.map((r) => r.id).sort());
    expect(res.results.length).toBeGreaterThan(0);
    expect(res.stats.total).toBe(rows.length);
    await client.close();
  });

  it('ids 子集只比较选中行', async () => {
    const { client, rows } = await openSession();
    const targets = pairedFiles(rows).slice(0, 2);
    const res = await compare(
      client,
      targets.map((r) => r.id),
    );
    expect(res.results.map((r) => r.id)).toEqual(targets.map((r) => r.id));
    await client.close();
  });

  it('五态结果 → 行状态映射与引擎一致，并经 dir/delta 回推', async () => {
    const { client, rows } = await openSession();
    const pushed: DirDelta[] = [];
    client.on('dir/delta', (p) => pushed.push(p as DirDelta));
    const res = await compare(client, [], 'rulesBased');
    for (const r of res.results) {
      const expected =
        r.result === 'binarySame' || r.result === 'rulesSame'
          ? 'same'
          : r.result === 'unimportantDiff'
            ? 'similar'
            : 'contentDiff';
      expect(r.status).toBe(expected);
    }
    const after = pushed.flatMap((d) => d.upserts);
    expect(after.length).toBeGreaterThan(0);
    for (const r of res.results) {
      const row = after.find((x) => x.id === r.id);
      expect(row?.status, `${r.id} 状态应回推`).toBe(r.status);
    }
    expect(rows.length).toBeGreaterThan(0);
    await client.close();
  });

  it('二进制方式只产出 binarySame/binaryDiff；规则方式不产出 binaryDiff', async () => {
    const { client } = await openSession();
    const bin = await compare(client, [], 'binary');
    for (const r of bin.results) expect(['binarySame', 'binaryDiff']).toContain(r.result);
    const rules = await compare(client, [], 'rulesBased');
    for (const r of rules.results) expect(r.result).not.toBe('binaryDiff');
    await client.close();
  });

  it('省略 method 时取会话判据的方法（设置 → dir/open → 比较）', async () => {
    const { client } = await openSession('binary');
    const implicit = await compare(client, []);
    const explicit = await compare(client, [], 'binary');
    expect(implicit.results).toEqual(explicit.results);
    const rules = await compare(client, [], 'rulesBased');
    expect(rules.results).not.toEqual(implicit.results);
    await client.close();
  });

  it('未知会话 → 410；文本会话 → 参数错误（不得静默成功）', async () => {
    const client = await newClient();
    await expect(
      client.request('dir/compareContent', { sessionId: 'nope', ids: [], method: null }),
    ).rejects.toMatchObject({ code: 410 });
    await client.request('text/open', { sessionId: 'txt', left: '/a.txt', right: '/b.txt' });
    await expect(
      client.request('dir/compareContent', { sessionId: 'txt', ids: [], method: null }),
    ).rejects.toThrow();
    await client.close();
  });
});
