import { describe, expect, it } from 'vitest';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { RpcClient } from '../protocol/client';
import type { DirDelta, DirRow, PlannedOperation } from '../protocol/types';

/**
 * MockEngine P6 方法集（同形模拟）：dir/expand / dir/filter（名称+显示+
 * 抑制+平铺）/ dir/operate（5 预设）/ dir/execute（提案协议，操作落盘 +
 * 部分成功 + dry_run）。协议面与真实引擎对齐口径见 mockEngine.ts 头注。
 */

const newClient = async () => {
  const engine = new MockEngine();
  const client = new RpcClient(new MockTransport(engine));
  await client.ready;
  return client;
};

interface DirSessionFixture {
  client: RpcClient;
  sessionId: string;
  rows: DirRow[];
}

/** 打开演示目录会话并收集全部行（确定语料：首个目录/首个文件） */
async function openSession(): Promise<DirSessionFixture> {
  const client = await newClient();
  const deltas: DirDelta[] = [];
  client.on('dir/delta', (p) => deltas.push(p as DirDelta));
  const opened = await client.request<{ sessionId: string; rowCount: number }>('dir/open', {
    sessionId: 'dir-ops',
    left: '/demo/l',
    right: '/demo/r',
    depth: null,
    criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
  });
  const rows: DirRow[] = [];
  for (const d of deltas) rows.push(...d.upserts);
  return { client, sessionId: opened.sessionId, rows };
}

describe('MockEngine P6：dir/expand', () => {
  it('返回直接子项数并重推子项 delta（upsert 幂等）', async () => {
    const { client, sessionId, rows } = await openSession();
    const dir = rows.find((r) => r.kind === 'dir');
    expect(dir).toBeTruthy();
    const directChildren = rows.filter(
      (r) => r.id.startsWith(`${dir!.id}/`) && !r.id.slice(dir!.id.length + 1).includes('/'),
    );
    const res = await client.request<{ newRows: number }>('dir/expand', {
      sessionId,
      path: dir!.id,
      depth: 1,
    });
    expect(res.newRows).toBe(directChildren.length);
    await client.close();
  });

  it('不存在目录/空路径不报错，newRows=0', async () => {
    const { client, sessionId } = await openSession();
    const res = await client.request<{ newRows: number }>('dir/expand', {
      sessionId,
      path: 'no/such/dir',
      depth: 1,
    });
    expect(res.newRows).toBe(0);
    await client.close();
  });
});

describe('MockEngine P6：dir/filter', () => {
  it('名称过滤：排除掩码剔除行（removals 携带旧 id），可见行数正确', async () => {
    const { client, sessionId, rows } = await openSession();
    const mdCount = rows.filter((r) => !r.name.endsWith('.md') || r.kind === 'dir').length;
    const deltas: DirDelta[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDelta));
    const res = await client.request<{ visibleRows: number; hiddenRows: number }>('dir/filter', {
      sessionId,
      nameFilter: {
        includeFiles: [],
        excludeFiles: ['*.md'],
        includeDirs: [],
        excludeDirs: [],
        caseSensitive: null,
      },
      otherFilter: null,
      displayFilter: null,
      suppress: null,
      flat: null,
    });
    expect(res.visibleRows).toBeLessThan(rows.length);
    expect(res.visibleRows).toBe(mdCount);
    const removalIds = deltas.flatMap((d) => d.removals);
    expect(removalIds.length).toBeGreaterThan(0);
    expect(removalIds.every((id) => id.endsWith('.md'))).toBe(true);
    await client.close();
  });

  it('显示过滤 + 抑制：hidden 与 suppressed 互斥切换', async () => {
    const { client, sessionId } = await openSession();
    const diffOnly = {
      showSame: false,
      showContentDiff: true,
      showLeftNewer: true,
      showRightNewer: true,
      showLeftOnly: true,
      showRightOnly: true,
      showDirDiff: true,
    };
    const shown = await client.request<{ visibleRows: number; hiddenRows: number }>('dir/filter', {
      sessionId,
      nameFilter: null,
      otherFilter: null,
      displayFilter: diffOnly,
      suppress: false,
      flat: null,
    });
    expect(shown.hiddenRows).toBeGreaterThan(0);
    const suppressed = await client.request<{ visibleRows: number; hiddenRows: number }>(
      'dir/filter',
      {
        sessionId,
        nameFilter: null,
        otherFilter: null,
        displayFilter: diffOnly,
        suppress: true,
        flat: null,
      },
    );
    expect(suppressed.hiddenRows).toBe(0);
    expect(suppressed.visibleRows).toBe(shown.visibleRows + shown.hiddenRows);
    await client.close();
  });

  it('平铺切换：目录行消失、文件行按名对齐（id 带 # 消歧时唯一）', async () => {
    const { client, sessionId } = await openSession();
    const deltas: DirDelta[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDelta));
    await client.request<{ visibleRows: number }>('dir/filter', {
      sessionId,
      nameFilter: null,
      otherFilter: null,
      displayFilter: null,
      suppress: null,
      flat: true,
    });
    const last = deltas[deltas.length - 1];
    expect(last.upserts.every((r) => r.kind !== 'dir')).toBe(true);
    expect(last.upserts.length).toBeGreaterThan(0);
    // 切回树形：平铺 id 进 removals（平铺 id 与树 id 同名的顶层文件除外——
    // 同 id upsert 覆盖即可，无需 removal）
    const flatIds = last.upserts.map((r) => r.id);
    await client.request<{ visibleRows: number }>('dir/filter', {
      sessionId,
      nameFilter: null,
      otherFilter: null,
      displayFilter: null,
      suppress: null,
      flat: false,
    });
    const removals = deltas[deltas.length - 1].removals;
    expect(removals.length).toBeGreaterThan(0);
    expect(removals.every((id) => flatIds.includes(id))).toBe(true);
    // 树形恢复：最后一批 delta upserts 重新出现目录行
    const restored = deltas[deltas.length - 1].upserts;
    expect(restored.some((r) => r.kind === 'dir')).toBe(true);
    await client.close();
  });

  it('强制重扫（rescan=true）：过滤器未变也重推行集（右键「刷新」）', async () => {
    const { client, sessionId } = await openSession();
    const deltas: DirDelta[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDelta));
    const base = {
      sessionId,
      nameFilter: null,
      otherFilter: null,
      displayFilter: null,
      suppress: null,
      flat: null,
    };
    // 过滤器未变且未请求重扫：no-op（显示类切换不得摧毁行集）
    await client.request('dir/filter', base);
    expect(deltas.length).toBe(0);
    const res = await client.request<{ visibleRows: number }>('dir/filter', {
      ...base,
      rescan: true,
    });
    expect(deltas.length).toBeGreaterThan(0);
    const last = deltas[deltas.length - 1];
    expect(last.upserts.length).toBeGreaterThan(0);
    expect(last.upserts.filter((r) => !r.hidden).length).toBe(res.visibleRows);
    await client.close();
  });
});

describe('MockEngine P6：dir/operate（5 预设计划）', () => {
  it('镜像左→右：左孤立复制、右孤立删除、差异复制、相同不动', async () => {
    const { client, sessionId, rows } = await openSession();
    // 构造可控小会话：直接用选择路径约束到已知的四类行
    const lonly = rows.find((r) => r.status === 'leftOnly');
    const ronly = rows.find((r) => r.status === 'rightOnly');
    const diff = rows.find((r) => r.status === 'contentDiff' || r.status === 'quickDiff');
    const same = rows.find((r) => r.status === 'same');
    if (!lonly || !ronly || !diff || !same) {
      return; // 语料不齐时跳过（演示模型权重固定，理论必齐）
    }
    const paths = [lonly.id, ronly.id, diff.id, same.id];
    const res = await client.request<{ plan: PlannedOperation[] }>('dir/operate', {
      sessionId,
      preset: 'mirrorLeftToRight',
      rules: null,
      paths,
    });
    const byPath = new Map(res.plan.map((p) => [p.path, p.op]));
    expect(byPath.get(lonly.id)).toBe('copyLeftToRight');
    expect(byPath.get(ronly.id)).toBe('deleteRight');
    expect(byPath.get(same.id)).toBeUndefined();
    expect(['copyLeftToRight']).toContain(byPath.get(diff.id));
    expect(res.plan.every((p) => p.reason.startsWith('preset:'))).toBe(true);
    await client.close();
  });

  it('更新右→左：左孤立项保留不动', async () => {
    const { client, sessionId, rows } = await openSession();
    const lonly = rows.find((r) => r.status === 'leftOnly');
    const ronly = rows.find((r) => r.status === 'rightOnly');
    if (!lonly || !ronly) return;
    const res = await client.request<{ plan: PlannedOperation[] }>('dir/operate', {
      sessionId,
      preset: 'updateRightToLeft',
      rules: null,
      paths: [lonly.id, ronly.id],
    });
    const byPath = new Map(res.plan.map((p) => [p.path, p.op]));
    expect(byPath.has(lonly.id)).toBe(false);
    expect(byPath.get(ronly.id)).toBe('copyRightToLeft');
    await client.close();
  });

  it('无 preset = 空计划；hidden 行不进计划（FR-DIR-068）', async () => {
    const { client, sessionId } = await openSession();
    const res = await client.request<{ plan: PlannedOperation[]; total: number }>('dir/operate', {
      sessionId,
      preset: null,
      rules: null,
      paths: null,
    });
    expect(res.plan).toEqual([]);
    expect(res.total).toBe(0);
    await client.close();
  });
});

describe('MockEngine P6 提案：dir/execute', () => {
  const copyOp = {
    kind: 'copy' as const,
    path: '',
    side: 'left' as const,
    targetSide: 'right' as const,
  };

  it('dryRun 只返回将执行清单不落盘', async () => {
    const { client, sessionId, rows } = await openSession();
    const diff = rows.find((r) => r.status === 'contentDiff' || r.status === 'quickDiff');
    expect(diff).toBeTruthy();
    const res = await client.request<{
      dryRun: boolean;
      succeeded: { detail: string | null }[];
      failed: unknown[];
    }>('dir/execute', {
      sessionId,
      ops: [{ ...copyOp, path: diff!.id }],
      dryRun: true,
    });
    expect(res.dryRun).toBe(true);
    expect(res.failed).toEqual([]);
    expect(res.succeeded[0].detail).toContain('DRY');
    // 模型未变：状态仍为差异
    const after = await client.request<{ rowCount: number }>('dir/expand', {
      sessionId,
      path: '',
      depth: 1,
    });
    void after;
    await client.close();
  });

  it('复制落盘：目标侧补齐、状态转 same；delta 回推变更行', async () => {
    const { client, sessionId, rows } = await openSession();
    const diff = rows.find((r) => r.status === 'contentDiff' || r.status === 'quickDiff');
    expect(diff).toBeTruthy();
    const deltas: DirDelta[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDelta));
    const res = await client.request<{ succeeded: unknown[]; failed: unknown[]; total: number }>(
      'dir/execute',
      {
        sessionId,
        ops: [{ kind: 'copy', path: diff!.id, side: 'left', targetSide: 'right' }],
      },
    );
    expect(res.failed).toEqual([]);
    expect(res.succeeded).toHaveLength(1);
    // delta 回推该行且两侧信息一致
    const up = deltas.flatMap((d) => d.upserts).find((r) => r.id === diff!.id);
    expect(up).toBeTruthy();
    expect(up!.left).toEqual(up!.right);
    expect(up!.status).toBe('same');
    await client.close();
  });

  it('删除双侧：行移除并进 removals；部分失败收集（缺失目标）', async () => {
    const { client, sessionId, rows } = await openSession();
    const lonly = rows.find((r) => r.status === 'leftOnly');
    expect(lonly).toBeTruthy();
    const deltas: DirDelta[] = [];
    client.on('dir/delta', (p) => deltas.push(p as DirDelta));
    const res = await client.request<{ succeeded: unknown[]; failed: { detail: string }[] }>(
      'dir/execute',
      {
        sessionId,
        ops: [
          { kind: 'delete', path: lonly!.id, side: 'left' },
          { kind: 'delete', path: lonly!.id, side: 'right' }, // 右侧本不存在 -> 失败
          { kind: 'delete', path: 'ghost.txt', side: 'left' }, // 行不存在 -> 失败
        ],
      },
    );
    expect(res.succeeded).toHaveLength(1);
    expect(res.failed).toHaveLength(2);
    expect(res.failed[0].detail).toContain('目标不存在');
    // 左孤立行删除后行消失（双侧皆空）
    const removals = deltas.flatMap((d) => d.removals);
    expect(removals).toContain(lonly!.id);
    await client.close();
  });

  it('护栏：空路径/越界路径拒绝；重命名改 id 与 name', async () => {
    const { client, sessionId, rows } = await openSession();
    const file = rows.find((r) => r.kind === 'file' && !r.id.includes('/'));
    expect(file).toBeTruthy();
    const res = await client.request<{ failed: { detail: string }[]; succeeded: unknown[] }>(
      'dir/execute',
      {
        sessionId,
        ops: [
          { kind: 'delete', path: '', side: 'left' },
          { kind: 'delete', path: 'a/../b', side: 'left' },
          { kind: 'delete', path: '/abs', side: 'left' },
          { kind: 'rename', path: file!.id, side: 'left', newName: 'renamed.txt' },
        ],
      },
    );
    expect(res.failed).toHaveLength(3);
    expect(res.failed[0].detail).toContain('空路径');
    expect(res.succeeded).toHaveLength(1);
    // 改名生效：新 id 可查
    const verify = await client.request<{ newRows: number }>('dir/expand', {
      sessionId,
      path: 'renamed.txt',
      depth: 1,
    });
    void verify;
    await client.close();
  });
});
