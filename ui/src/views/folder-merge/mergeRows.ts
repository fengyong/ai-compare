/**
 * 文件夹三路合并 UI 数据面（FR-DMRG-001~003，纯函数）：
 * - 平铺 DirMergeRow 构树/排序（DFS 序，父先于子）；
 * - 行状态着色口径（FR-DMRG-002：绿=可自动 / 红=冲突 / 灰=孤立 / 暗=一致）；
 * - 决策状态机：DecisionMap（rel -> 决策），有效决策 = 显式 > 祖先继承 >
 *   auto；目录决策经 applyDecisionToTree 递归展开（子项显式决策被覆盖）；
 * - 决策聚合统计（冲突解决进度/待处理计数）；
 * - 计划预览行（join 引擎 dir/mergePlan 的 ops，不镜像默认决策表）；
 * - 合并报告文本导出。
 */

import type {
  DirMergeCategory,
  DirMergeExecuteResult,
  DirMergeRow,
  DirMergeRowState,
  DirMergeDecision,
  DirMergeOpKind,
  DirMergePlannedOp,
  DirMergeTarget,
} from '../../protocol/types';

// ------------------------------------------------------------------
// 树/排序
// ------------------------------------------------------------------

/** DFS 序可见行（父先于子；展开状态折叠子树） */
export function orderedMergeRows(
  rows: readonly DirMergeRow[],
  expanded: ReadonlySet<string>,
): DirMergeRow[] {
  const childrenOf = new Map<string | null, DirMergeRow[]>();
  for (const r of rows) {
    const list = childrenOf.get(r.parent) ?? [];
    list.push(r);
    childrenOf.set(r.parent, list);
  }
  for (const list of childrenOf.values()) {
    list.sort((a, b) =>
      a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'dir' ? -1 : 1,
    );
  }
  const out: DirMergeRow[] = [];
  const walk = (parent: string | null): void => {
    for (const row of childrenOf.get(parent) ?? []) {
      out.push(row);
      if (row.kind === 'dir' && expanded.has(row.relPath)) walk(row.relPath);
    }
  };
  walk(null);
  return out;
}

/** 目录行是否可展开（有子项；类型冲突叶/空目录不可展开） */
export function hasMergeChildren(rows: readonly DirMergeRow[], relPath: string): boolean {
  return rows.some((r) => r.parent === relPath);
}

/** 默认展开集：全部目录（合并树通常全展开；折叠为用户后续操作） */
export function defaultExpanded(rows: readonly DirMergeRow[]): Set<string> {
  return new Set(rows.filter((r) => r.kind === 'dir').map((r) => r.relPath));
}

// ------------------------------------------------------------------
// 着色/展示口径
// ------------------------------------------------------------------

/** 行状态 -> 着色 class（FR-DMRG-002 合并色映射） */
export function mergeStateClass(state: DirMergeRowState): string {
  switch (state) {
    case 'auto':
      return 'merge-auto';
    case 'conflict':
      return 'merge-conflict';
    case 'orphan':
      return 'merge-orphan';
    default:
      return 'merge-same';
  }
}

/** 目录行聚合状态文案（避免"仅左侧存在"这类叶分类误标目录聚合行） */
export const MERGE_STATE_LABELS: Record<DirMergeRowState, string> = {
  same: '子树一致',
  auto: '子树可自动合并',
  conflict: '子树含冲突',
  orphan: '子树含孤立项',
};

export const MERGE_CATEGORY_LABELS: Record<DirMergeCategory, string> = {
  same: '三方一致',
  leftChanged: '仅左侧修改',
  rightChanged: '仅右侧修改',
  bothSame: '两侧同改',
  deletedRight: '右侧已删除',
  deletedLeft: '左侧已删除',
  conflict: '冲突需人工',
  leftOnly: '仅左侧存在',
  rightOnly: '仅右侧存在',
  baseOnly: '两侧均已删除',
};

// ------------------------------------------------------------------
// 决策状态机
// ------------------------------------------------------------------

/** 决策表（rel 路径 -> 显式决策） */
export type DecisionMap = Record<string, DirMergeDecision>;

/** 祖先链（rel -> 最近祖先在前的数组；不含自身） */
function ancestorChain(relPath: string): string[] {
  const chain: string[] = [];
  let cur = relPath.includes('/') ? relPath.slice(0, relPath.lastIndexOf('/')) : null;
  while (cur) {
    chain.push(cur);
    cur = cur.includes('/') ? cur.slice(0, cur.lastIndexOf('/')) : null;
  }
  return chain;
}

/** 有效决策 = 自身显式 > 最近祖先显式 > auto */
export function effectiveDecision(
  relPath: string,
  decisions: Readonly<DecisionMap>,
): DirMergeDecision {
  const own = decisions[relPath];
  if (own) return own;
  for (const anc of ancestorChain(relPath)) {
    const d = decisions[anc];
    if (d) return d;
  }
  return 'auto';
}

/** 设置决策；目录决策传播到全部后代（覆盖后代既有显式决策，交互可预期） */
export function applyDecisionToTree(
  rows: readonly DirMergeRow[],
  decisions: DecisionMap,
  relPath: string,
  decision: DirMergeDecision,
): DecisionMap {
  const next: DecisionMap = { ...decisions };
  const isDir = rows.find((r) => r.relPath === relPath)?.kind === 'dir';
  next[relPath] = decision;
  if (isDir) {
    const prefix = `${relPath}/`;
    for (const r of rows) {
      if (r.relPath.startsWith(prefix)) next[r.relPath] = decision;
    }
  }
  if (decision === 'auto') {
    // 回到按分类默认：清除该行（及目录子树）的显式决策
    delete next[relPath];
    if (isDir) {
      const prefix = `${relPath}/`;
      for (const r of rows) {
        if (r.relPath.startsWith(prefix)) delete next[r.relPath];
      }
    }
  }
  return next;
}

/** 目录决策是否影响子树（交互提示用） */
export function decisionAffectsSubtree(rows: readonly DirMergeRow[], relPath: string): boolean {
  return rows.find((r) => r.relPath === relPath)?.kind === 'dir' && hasMergeChildren(rows, relPath);
}

/** 决策聚合统计（信息面板：冲突解决进度） */
export interface MergeDecisionStats {
  conflicts: number;
  conflictsResolved: number;
  orphans: number;
  explicitDecisions: number;
  skipDecisions: number;
}

export function mergeDecisionStats(
  rows: readonly DirMergeRow[],
  decisions: Readonly<DecisionMap>,
): MergeDecisionStats {
  let conflicts = 0;
  let conflictsResolved = 0;
  let orphans = 0;
  let skipDecisions = 0;
  let explicitDecisions = 0;
  for (const r of rows) {
    if (r.rowState === 'conflict') conflicts++;
    if (r.rowState === 'orphan') orphans++;
    const d = decisions[r.relPath];
    if (d) {
      explicitDecisions++;
      if (d === 'skip') skipDecisions++;
      if (r.rowState === 'conflict' && (d === 'takeLeft' || d === 'takeRight')) conflictsResolved++;
    }
    // 继承决策也算"已解决"（目录决策覆盖下的冲突行）
    if (r.rowState === 'conflict' && !d && effectiveDecision(r.relPath, decisions) !== 'auto')
      conflictsResolved++;
  }
  return { conflicts, conflictsResolved, orphans, explicitDecisions, skipDecisions };
}

// ------------------------------------------------------------------
// 计划预览（引擎 plan_merge 默认决策表的客户端镜像）
// ------------------------------------------------------------------

/** 预览行：决策/默认 -> 动作 */
export interface MergePreviewRow {
  path: string;
  name: string;
  kind: DirMergeRow['kind'];
  category: DirMergeCategory;
  decision: DirMergeDecision;
  op: DirMergeOpKind;
  reason: string;
  warning: boolean;
  /** 动作是否真的会执行（keep 仅为解释性占位） */
  actionable: boolean;
}

/**
 * 计划预览行：以引擎 dir/mergePlan 返回的 ops 为唯一权威（动作/理由均取引擎），
 * 仅 join 扫描行的元数据与本地有效决策用于展示。客户端不再镜像默认决策表，
 * 避免与引擎 auto_default/walk_plan 漂移后执行未经引擎计划的操作。
 */
export function mergePreviewFromPlan(
  rows: readonly DirMergeRow[],
  decisions: Readonly<DecisionMap>,
  ops: readonly DirMergePlannedOp[],
): MergePreviewRow[] {
  const byPath = new Map(rows.map((r) => [r.relPath, r]));
  return ops.map((op) => {
    const row = byPath.get(op.path);
    return {
      path: op.path,
      name: row?.name ?? op.path,
      kind: row?.kind ?? 'file',
      category: row?.category ?? 'same',
      decision: effectiveDecision(op.path, decisions),
      op: op.kind,
      reason: op.reason,
      warning: op.kind === 'deleteTarget',
      actionable: op.kind !== 'keep',
    };
  });
}

/** 预览统计（确认框统计行；与引擎 plan stats 同口径） */
export interface MergePlanStats {
  takeLeft: number;
  takeRight: number;
  deleteTarget: number;
  keep: number;
  total: number;
}

export function summarizeMergePlan(preview: readonly MergePreviewRow[]): MergePlanStats {
  const s: MergePlanStats = {
    takeLeft: 0,
    takeRight: 0,
    deleteTarget: 0,
    keep: 0,
    total: preview.length,
  };
  for (const p of preview) {
    if (p.op === 'keep') s.keep++;
    else if (p.op === 'takeLeft') s.takeLeft++;
    else if (p.op === 'takeRight') s.takeRight++;
    else s.deleteTarget++;
  }
  return s;
}

/** 导出文本报告（DMRG 信息面板/日志面板共用） */
export function exportMergeReportText(
  meta: { target: DirMergeTarget; base: string; left: string; right: string },
  result: Pick<
    DirMergeExecuteResult,
    'total' | 'copied' | 'deleted' | 'skipped' | 'failed' | 'cancelled' | 'details'
  >,
): string {
  const lines: string[] = [];
  lines.push('ai-compare 文件夹合并报告');
  lines.push(
    `合并目标：${meta.target === 'left' ? '左侧' : meta.target === 'right' ? '右侧' : '输出文件夹'}`,
  );
  lines.push(`基准：${meta.base}`);
  lines.push(`左侧：${meta.left}`);
  lines.push(`右侧：${meta.right}`);
  lines.push('');
  lines.push(
    `统计：共 ${result.total} 项 — 复制 ${result.copied} / 删除 ${result.deleted} / 跳过 ${result.skipped} / 失败 ${result.failed}${result.cancelled ? '（已取消）' : ''}`,
  );
  lines.push('');
  lines.push('明细：');
  for (const d of result.details) {
    lines.push(`  [${d.status}] ${d.kind} ${d.path}${d.detail ? ` — ${d.detail}` : ''}`);
  }
  return lines.join('\n');
}
