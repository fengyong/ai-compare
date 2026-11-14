import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type { DirMergeRow, DirMergeDecision, DirMergePlannedOp, DirMergeTarget } from '../protocol/types';

/**
 * MockEngine P8 方法集（同形模拟）：dir/mergeScan（三方演示树分类）/
 * dir/mergePlan（决策继承 + 默认决策表）/ dir/mergeExecute（落盘 +
 * 部分成功 + 日志）。协议面与引擎 merge3way 同口径，见 mockEngine.ts 头注。
 */

const newClient = async () => {
  const engine = new MockEngine();
  const client = new RpcClient(new MockTransport(engine));
  await client.ready;
  return client;
};

async function openMerge(client: RpcClient, sessionId = 'merge-1'): Promise<DirMergeRow[]> {
  const res = await client.request<{ rows: DirMergeRow[] }>('dir/mergeScan', {
    sessionId,
    base: '/demo/base',
    left: '/demo/left',
    right: '/demo/right',
  });
  return res.rows;
}

const find = (rows: DirMergeRow[], rel: string): DirMergeRow => rows.find((r) => r.relPath === rel)!;

describe('MockEngine P8：dir/mergeScan', () => {
  it('演示树覆盖全部分类形态', async () => {
    const client = await newClient();
    const rows = await openMerge(client);
    expect(find(rows, 'README.md').category).toBe('same');
    expect(find(rows, 'version.json').category).toBe('leftChanged');
    expect(find(rows, 'config.yml').category).toBe('rightChanged');
    expect(find(rows, 'conflict.c').category).toBe('conflict');
    expect(find(rows, 'conflict.c').conflictLeaf).toBe(true);
    expect(find(rows, 'both.c').category).toBe('bothSame');
    expect(find(rows, 'deleted-r.txt').category).toBe('deletedRight');
    expect(find(rows, 'deleted-mod.txt').category).toBe('conflict');
    expect(find(rows, 'lonly.txt').category).toBe('leftOnly');
    expect(find(rows, 'ronly.txt').category).toBe('rightOnly');
    expect(find(rows, 'both-new.txt').category).toBe('bothSame');
    expect(find(rows, 'gone.txt').category).toBe('baseOnly');
    expect(find(rows, 'clash').category).toBe('conflict');
    await client.close();
  });

  it('目录行聚合冲突/孤立，行含 parent/depth', async () => {
    const client = await newClient();
    const rows = await openMerge(client);
    const src = find(rows, 'src');
    expect(src.kind).toBe('dir');
    // 子树含孤立目录（orphan-dir/empty-orphan）→ 聚合为灰
    expect(src.rowState).toBe('orphan');
    expect(src.depth).toBe(1);
    const inner = find(rows, 'src/orphan-dir/inner.txt');
    expect(inner.parent).toBe('src/orphan-dir');
    expect(inner.depth).toBe(3);
    expect(inner.rowState).toBe('orphan');
    await client.close();
  });

  it('重复扫描幂等（刷新/StrictMode 双调用安全）；stats 计数一致', async () => {
    const client = await newClient();
    await openMerge(client);
    // 同会话重扫描：幂等重建（不报 411）
    const rescanned = await client.request<{ rows: DirMergeRow[] }>('dir/mergeScan', {
      sessionId: 'merge-1',
      base: '/demo/base',
      left: '/demo/left',
      right: '/demo/right',
    });
    expect(rescanned.rows).toHaveLength(18);
    const rows = await openMerge(client, 'merge-2');
    const res = await client.request<{ stats: { total: number; conflict: number; orphan: number; auto: number; same: number } }>('dir/mergeScan', {
      sessionId: 'merge-3',
      base: '/demo/base',
      left: '/demo/left',
      right: '/demo/right',
    });
    expect(res.stats.total).toBe(rows.length);
    expect(res.stats.conflict).toBe(rows.filter((r) => r.rowState === 'conflict').length);
    expect(res.stats.orphan).toBe(rows.filter((r) => r.rowState === 'orphan').length);
    expect(res.stats.auto).toBe(rows.filter((r) => r.rowState === 'auto').length);
    await client.close();
  });
});

describe('MockEngine P8：dir/mergePlan', () => {
  it('默认决策（目标右）：左改/左孤立取左，冲突保持，右改不动', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      decisions: {},
    });
    const ops = res.ops;
    expect(ops.find((o) => o.path === 'version.json')?.kind).toBe('takeLeft');
    expect(ops.find((o) => o.path === 'lonly.txt')?.kind).toBe('takeLeft');
    expect(ops.find((o) => o.path === 'config.yml')).toBeUndefined(); // 右改在目标右 = 不动（Auto Keep 不入计划）
    expect(ops.find((o) => o.path === 'conflict.c')).toBeUndefined(); // 冲突默认人工
    expect(ops.find((o) => o.path === 'README.md')).toBeUndefined(); // 三方一致
    expect(ops.some((o) => o.path === 'src/main.c' && o.kind === 'takeLeft')).toBe(true);
    await client.close();
  });

  it('显式决策取左覆盖冲突默认；skip 决策产生 keep 记录', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      decisions: { 'conflict.c': 'takeLeft', 'both.c': 'skip' } as Record<string, DirMergeDecision>,
    });
    expect(res.ops.find((o) => o.path === 'conflict.c')?.kind).toBe('takeLeft');
    const skipOp = res.ops.find((o) => o.path === 'both.c');
    expect(skipOp?.kind).toBe('keep');
    expect(skipOp?.reason).toContain('跳过');
    await client.close();
  });

  it('目标左：右改取右；右孤立并入', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'left' as DirMergeTarget,
      decisions: {},
    });
    expect(res.ops.find((o) => o.path === 'config.yml')?.kind).toBe('takeRight');
    expect(res.ops.find((o) => o.path === 'ronly.txt')?.kind).toBe('takeRight');
    expect(res.ops.find((o) => o.path === 'version.json')).toBeUndefined();
    await client.close();
  });

  it('目录决策继承：src 决策 takeRight 时子项全部 takeRight（目标左）', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'left' as DirMergeTarget,
      decisions: { src: 'takeRight' } as Record<string, DirMergeDecision>,
    });
    expect(res.ops.find((o) => o.path === 'src/main.c')?.kind).toBe('takeRight');
    expect(res.ops.find((o) => o.path === 'src/util.c')?.kind).toBe('takeRight');
    // inner.txt 仅左侧存在：继承 takeRight 但右侧缺失 → keep（缺失保持）
    const innerOp = res.ops.find((o) => o.path === 'src/orphan-dir/inner.txt');
    expect(innerOp?.kind).toBe('keep');
    expect(innerOp?.reason).toContain('缺失');
    await client.close();
  });

  it('空孤立目录并入计划；非空孤立目录由子项承载', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      decisions: {},
    });
    expect(res.ops.find((o) => o.path === 'src/empty-orphan')?.kind).toBe('takeLeft');
    expect(res.ops.find((o) => o.path === 'src/orphan-dir')).toBeUndefined();
    expect(res.ops.find((o) => o.path === 'src/orphan-dir/inner.txt')?.kind).toBe('takeLeft');
    await client.close();
  });
});

describe('MockEngine P8：dir/mergeExecute', () => {
  it('takeLeft 覆盖目标侧并重算状态；部分成功收集失败项', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{
      copied: number;
      failed: number;
      details: { path: string; status: string }[];
    }>('dir/mergeExecute', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      ops: [
        { kind: 'takeLeft', path: 'version.json', reason: 'test' },
        { kind: 'takeLeft', path: 'nonexistent', reason: 'test' },
      ],
    });
    expect(res.copied).toBe(1);
    expect(res.failed).toBe(1);
    expect(res.details.find((d) => d.path === 'nonexistent')?.status).toBe('failed');
    // 模型已落盘：重计划后 version.json 变三方一致（不再出现在动作计划）
    const replan = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      decisions: {},
    });
    expect(replan.ops.find((o) => o.path === 'version.json')).toBeUndefined();
    await client.close();
  });

  it('deleteTarget 在目标右删除已存在项；目标缺失记 skipped', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{
      deleted: number;
      skipped: number;
    }>('dir/mergeExecute', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      ops: [
        { kind: 'deleteTarget', path: 'ronly.txt', reason: 'test' },
        { kind: 'deleteTarget', path: 'lonly.txt', reason: 'test' },
      ],
    });
    expect(res.deleted).toBe(1);
    expect(res.skipped).toBe(1);
    // 模型已落盘：重计划后 ronly.txt 右删成为 deletedRight（目标右 = 不动）
    const replan = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      decisions: {},
    });
    expect(replan.ops.find((o) => o.path === 'ronly.txt')).toBeUndefined();
    await client.close();
  });

  it('keep 项记 planned；日志行含统计', async () => {
    const client = await newClient();
    await openMerge(client);
    const res = await client.request<{ skipped: number; log: string[] }>('dir/mergeExecute', {
      sessionId: 'merge-1',
      target: 'right' as DirMergeTarget,
      ops: [{ kind: 'keep', path: 'README.md', reason: '冲突需人工处理' }],
    });
    expect(res.skipped).toBe(1);
    expect(res.log.some((l) => l.includes('合并完成'))).toBe(true);
    await client.close();
  });

  it('未扫描会话报 410', async () => {
    const client = await newClient();
    await expect(
      client.request('dir/mergePlan', { sessionId: 'nope', target: 'right', decisions: {} }),
    ).rejects.toMatchObject({ code: 410 });
    await client.close();
  });
});

describe('MockEngine P8：输出目标（第三目录）契约', () => {
  it('target=output 缺 outputPath：plan 与 execute 均拒绝（不得静默写到别处）', async () => {
    const client = await newClient();
    await openMerge(client);
    await expect(
      client.request('dir/mergePlan', { sessionId: 'merge-1', target: 'output', decisions: {} }),
    ).rejects.toMatchObject({ code: -32602 });
    await expect(
      client.request('dir/mergeExecute', {
        sessionId: 'merge-1',
        target: 'output',
        ops: [{ kind: 'takeLeft', path: 'version.json', reason: 'test' }],
      }),
    ).rejects.toMatchObject({ code: -32602 });
    await client.close();
  });

  it('plan 带 outputPath 记入会话：execute 省略仍可用', async () => {
    const client = await newClient();
    await openMerge(client);
    const plan = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'output',
      decisions: {},
      outputPath: '/demo/out',
    });
    expect(plan.ops.length).toBeGreaterThan(0);
    const res = await client.request<{ copied: number; failed: number; target: string }>('dir/mergeExecute', {
      sessionId: 'merge-1',
      target: 'output',
      ops: plan.ops.filter((o) => o.kind !== 'keep'),
    });
    expect(res.target).toBe('output');
    expect(res.failed).toBe(0);
    expect(res.copied).toBe(plan.ops.filter((o) => o.kind === 'takeLeft' || o.kind === 'takeRight').length);
    await client.close();
  });

  it('输出目标：三方一致也复制，冲突项不进输出', async () => {
    const client = await newClient();
    await openMerge(client);
    const plan = await client.request<{ ops: DirMergePlannedOp[] }>('dir/mergePlan', {
      sessionId: 'merge-1',
      target: 'output',
      decisions: {},
      outputPath: '/demo/out',
    });
    expect(plan.ops.find((o) => o.path === 'README.md')?.kind).toBe('takeLeft');
    expect(plan.ops.find((o) => o.path === 'conflict.c')).toBeUndefined();
    await client.close();
  });
});
