/**
 * 文件夹三路合并 UI 数据面（FR-DMRG-001~003，纯函数）：
 * - 平铺 DirMergeRow 构树/排序（DFS 序，父先于子）；
 * - 行状态着色口径（FR-DMRG-002：绿=可自动 / 红=冲突 / 灰=孤立 / 暗=一致）；
 * - 决策状态机：DecisionMap（rel -> 决策），有效决策 = 显式 > 祖先继承 >
 *   auto；目录决策经 applyDecisionToTree 递归展开（子项显式决策被覆盖）；
 * - 决策聚合统计（冲突解决进度/待处理计数）；
 * - 计划预览行（与引擎 plan_merge 默认决策表同口径的客户端镜像；
 *   协议携带决策后迁回引擎 dir/mergePlan，见 P8 RPC 变更清单）；
 * - 计划 -> DirExecuteOp（经 dir/execute 同形协议执行）与文本报告导出。
 */

import type { DirMergeCategory, DirMergeRow, DirMergeRowState, MergeDecision, MergeOpKind, MergePlannedOp, MergeTarget } from '../../protocol/types';

// ------------------------------------------------------------------
// 树/排序
// ------------------------------------------------------------------

/** DFS 序可见行（父先于子；展开状态折叠子树） */
export function orderedMergeRows(rows: readonly DirMergeRow[], expanded: ReadonlySet<string>): DirMergeRow[] {
  const childrenOf = new Map<string | null, DirMergeRow[]>();
  for (const r of rows) {
    const list = childrenOf.get(r.parent) ?? [];
    list.push(r);
    childrenOf.set(r.parent, list);
  }
  for (const list of childrenOf.values()) {
    list.sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'dir' ? -1 : 1));
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
export type DecisionMap = Record<string, MergeDecision>;

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
export function effectiveDecision(relPath: string, decisions: Readonly<DecisionMap>): MergeDecision {
  const own = decisions[relPath];
  if (own) return own;
  for (const anc of ancestorChain(relPath)) {
    const d = decisions[anc];
    if (d) return d;
  }
  return 'auto';
}

/** 设置决策；目录决策传播到全部后代（覆盖后代既有显式决策，交互可预期） */
export function applyDecisionToTree(rows: readonly DirMergeRow[], decisions: DecisionMap, relPath: string, decision: MergeDecision): DecisionMap {
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

export function mergeDecisionStats(rows: readonly DirMergeRow[], decisions: Readonly<DecisionMap>): MergeDecisionStats {
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
    if (r.rowState === 'conflict' && !d && effectiveDecision(r.relPath, decisions) !== 'auto') conflictsResolved++;
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
  decision: MergeDecision;
  op: MergeOpKind;
  reason: string;
  warning: boolean;
  /** 动作是否真的会执行（keep 仅为解释性占位） */
  actionable: boolean;
}

/** Auto 默认决策表（与引擎 merge3way::auto_default 同口径） */
export function autoDefaultOp(row: DirMergeRow, target: MergeTarget): { op: MergeOpKind; reason: string } {
  const cat = row.category;
  switch (cat) {
    case 'same':
      return target === 'output' ? { op: 'takeLeft', reason: '三方一致（输出取左）' } : { op: 'keep', reason: '三方一致' };
    case 'leftChanged':
      return target === 'left' ? { op: 'keep', reason: '目标侧即左侧（已含修改）' } : { op: 'takeLeft', reason: '仅左改 → 取左' };
    case 'rightChanged':
      return target === 'right' ? { op: 'keep', reason: '目标侧即右侧（已含修改）' } : { op: 'takeRight', reason: '仅右改 → 取右' };
    case 'bothSame':
      return target === 'output' ? { op: 'takeLeft', reason: '两侧同改一致（输出取左，等价取右）' } : { op: 'keep', reason: '两侧同改一致' };
    case 'deletedRight':
      return target === 'right'
        ? { op: 'keep', reason: '目标侧已删除' }
        : target === 'left'
          ? { op: 'deleteTarget', reason: '跟随右侧删除' }
          : { op: 'keep', reason: '右侧已删除（输出不含该项）' };
    case 'deletedLeft':
      return target === 'left'
        ? { op: 'keep', reason: '目标侧已删除' }
        : target === 'right'
          ? { op: 'deleteTarget', reason: '跟随左侧删除' }
          : { op: 'keep', reason: '左侧已删除（输出不含该项）' };
    case 'conflict':
      return { op: 'keep', reason: '冲突需人工处理' };
    case 'leftOnly':
      return target === 'left' ? { op: 'keep', reason: '目标侧即左侧' } : { op: 'takeLeft', reason: target === 'output' ? '左孤立并入输出' : '左孤立并入目标' };
    case 'rightOnly':
      return target === 'right' ? { op: 'keep', reason: '目标侧即右侧' } : { op: 'takeRight', reason: target === 'output' ? '右孤立并入输出' : '右孤立并入目标' };
    case 'baseOnly':
      return { op: 'keep', reason: '两侧均已删除' };
  }
}

/** 全行预览（决策展开 + 默认表；供计划确认对话框/统计） */
export function buildMergePreview(rows: readonly DirMergeRow[], decisions: Readonly<DecisionMap>, target: MergeTarget): MergePreviewRow[] {
  const out: MergePreviewRow[] = [];
  for (const row of rows) {
    const decision = effectiveDecision(row.relPath, decisions);
    let op: MergeOpKind;
    let reason: string;
    switch (decision) {
      case 'skip':
        op = 'keep';
        reason = '决策:跳过';
        break;
      case 'takeLeft':
        if (!row.left) {
          op = 'keep';
          reason = '决策:取左（左侧缺失，保持现状）';
        } else if (target === 'left') {
          op = 'keep';
          reason = '决策:取左（目标侧即左侧）';
        } else {
          op = 'takeLeft';
          reason = '决策:取左';
        }
        break;
      case 'takeRight':
        if (!row.right) {
          op = 'keep';
          reason = '决策:取右（右侧缺失，保持现状）';
        } else if (target === 'right') {
          op = 'keep';
          reason = '决策:取右（目标侧即右侧）';
        } else {
          op = 'takeRight';
          reason = '决策:取右';
        }
        break;
      case 'both': {
        if (row.kind === 'dir') {
          op = 'keep';
          reason = '决策:both（目录递归合并，子项按各自决策）';
        } else {
          const d = autoDefaultOp(row, target);
          op = d.op;
          reason = `决策:both（单文件降级默认）· ${d.reason}`;
        }
        break;
      }
      default: {
        const d = autoDefaultOp(row, target);
        op = d.op;
        reason = d.reason;
      }
    }
    out.push({
      path: row.relPath,
      name: row.name,
      kind: row.kind,
      category: row.category,
      decision,
      op,
      reason,
      warning: op === 'deleteTarget',
      actionable: op !== 'keep',
    });
  }
  return out;
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
  const s: MergePlanStats = { takeLeft: 0, takeRight: 0, deleteTarget: 0, keep: 0, total: preview.length };
  for (const p of preview) {
    if (p.op === 'keep') s.keep++;
    else if (p.op === 'takeLeft') s.takeLeft++;
    else if (p.op === 'takeRight') s.takeRight++;
    else s.deleteTarget++;
  }
  return s;
}

/** 预览动作行 -> MergePlannedOp（经 dir/mergeExecute 或 dir/execute 执行） */
export function previewToMergeOps(preview: readonly MergePreviewRow[]): MergePlannedOp[] {
  return preview
    .filter((p) => p.actionable)
    .map((p) => ({ kind: p.op, path: p.path, reason: p.reason }));
}

/** 导出文本报告（DMRG 信息面板/日志面板共用） */
export function exportMergeReportText(
  meta: { target: MergeTarget; base: string; left: string; right: string },
  result: { total: number; copied: number; deleted: number; skipped: number; failed: number; cancelled: boolean; details: { kind: string; path: string; status: string; detail: string | null }[] },
): string {
  const lines: string[] = [];
  lines.push('ai-compare 文件夹合并报告');
  lines.push(`合并目标：${meta.target === 'left' ? '左侧' : meta.target === 'right' ? '右侧' : '输出文件夹'}`);
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
