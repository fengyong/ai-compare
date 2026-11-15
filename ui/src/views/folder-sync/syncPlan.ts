/**
 * 文件夹同步计划编排（FR-SYNC-002/003/004 的 UI 侧数据面，纯函数）：
 * - 同步策略（SYNC-004 预设内可配）：删除孤立项 / 时间戳冲突策略 /
 *   只读侧跳过 / 同步空文件夹 —— 在引擎 dir/operate 计划上做客户端施加
 *   （与 engine/ai-dir sync.rs apply_strategy 同语义；协议携带策略字段后
 *   该变换迁回引擎，见 P8 交付报告 RPC 变更清单）；
 * - 逐项操作改写（FR-SYNC-003 操作菜单：保持不变/复制/删除）；
 * - 计划预览行状态机（方向着色/删除警示/跳过标记）与统计聚合；
 * - 结果报告文本导出（FR-SYNC 报告/日志需求）。
 */

import type { DirExecuteOp, DirRow, PlannedOperation, PlannedOpKind } from '../../protocol/types';
import { plannedToOp } from '../folder-compare/dirOps';

/** 同步策略（UI 面；与引擎 SyncStrategy 同语义） */
export interface SyncStrategyUi {
  /** 删除孤立项（镜像预设）：false = 保留对侧孤立项 */
  deleteOrphans: boolean;
  /** 时间戳冲突策略：newerWins=较新优先（相等取左）/ leftWins=左侧优先 */
  leftWins: boolean;
  /** 只读侧：该侧写入操作（复制进/删除）跳过 */
  readonlySide: 'none' | 'left' | 'right';
  /** 同步空文件夹：false = 孤立空目录跳过 */
  syncEmptyDirs: boolean;
}

export const DEFAULT_SYNC_STRATEGY: SyncStrategyUi = {
  deleteOrphans: true,
  leftWins: false,
  readonlySide: 'none',
  syncEmptyDirs: true,
};

/** 逐项操作改写（FR-SYNC-003 操作菜单；auto = 预设计划原样） */
export type RowOpOverride =
  'auto' | 'keep' | 'copyLeftToRight' | 'copyRightToLeft' | 'deleteLeft' | 'deleteRight';

export const ROW_OVERRIDE_LABELS: Record<RowOpOverride, string> = {
  auto: '按预设',
  keep: '保持不变',
  copyLeftToRight: '复制到右',
  copyRightToLeft: '复制到左',
  deleteLeft: '删除左侧',
  deleteRight: '删除右侧',
};

/** 配对差异行（策略冲突判定与引擎 is_diff 同口径） */
function isDiffRow(row: DirRow): boolean {
  return row.status === 'contentDiff' || row.status === 'quickDiff' || row.status === 'dirDiff';
}

function writesTo(op: PlannedOpKind, side: 'left' | 'right'): boolean {
  if (side === 'right') {
    return op === 'copyLeftToRight' || op === 'deleteRight' || op === 'touchRight';
  }
  return op === 'copyRightToLeft' || op === 'deleteLeft' || op === 'touchLeft';
}

/**
 * 策略施加（与引擎 apply_strategy 同语义的客户端镜像）：
 * 只读侧跳过 > 左侧优先改向 > 删除孤儿开关 > 空目录开关。
 * 空目录判定：目录行的"无子项且单侧存在"按 UI 行集近似（叶子目录行 id 无 `/` 子项）。
 */
export function applyStrategyToPlan(
  plan: PlannedOperation[],
  rows: readonly DirRow[],
  strategy: SyncStrategyUi,
): { ops: PlannedOperation[]; skipped: PlannedOperation[] } {
  const byId = new Map(rows.map((r) => [r.id, r]));
  const isLeafOrphanDir = (row: DirRow): boolean => {
    if (row.status !== 'leftOnly' && row.status !== 'rightOnly') return false;
    // R3-03：按真实 rel 路径判层级；平铺行 id 是 name/name#i，没有 `/` 层级，
    // 用 id 做前缀判断会把所有平铺目录误判为空目录而跳过。
    const rel = row.leftPath ?? row.rightPath;
    if (!rel) return true;
    const prefix = `${rel}/`;
    for (const r of rows) {
      if (r.leftPath?.startsWith(prefix) || r.rightPath?.startsWith(prefix)) return false;
    }
    return true;
  };
  const ops: PlannedOperation[] = [];
  const skipped: PlannedOperation[] = [];
  for (const base of plan) {
    const row = byId.get(base.path);
    let op = base;

    // 1. 只读侧跳过
    if (strategy.readonlySide !== 'none' && writesTo(op.op, strategy.readonlySide)) {
      skipped.push({
        ...op,
        reason: `${op.reason}; strategy:readonlySide(${strategy.readonlySide})`,
      });
      continue;
    }

    // 2. 冲突策略：左侧优先时差异行的右→左复制改向
    if (strategy.leftWins && op.op === 'copyRightToLeft' && row && isDiffRow(row)) {
      // 改向不只是换枚举：真实源/目标侧地址必须一起换向，否则会把右侧
      // 内容写回左侧（R3-03）。异名对齐时两侧 rel 不同，按 row 重新解析。
      const lp = row.leftPath ?? op.targetPath;
      const rp = row.rightPath ?? op.sourcePath;
      const target = rp ?? lp;
      op = {
        ...op,
        op: 'copyLeftToRight',
        reason: `${op.reason}; strategy:leftWins`,
        sourcePath: lp,
        targetPath: target,
        operationPath: undefined,
        displayPath: target ?? op.displayPath,
      };
    }

    // 3. 删除孤儿开关
    if (
      !strategy.deleteOrphans &&
      (op.op === 'deleteLeft' || op.op === 'deleteRight') &&
      row &&
      (row.status === 'leftOnly' || row.status === 'rightOnly')
    ) {
      skipped.push({ ...op, reason: `${op.reason}; strategy:keepOrphans` });
      continue;
    }

    // 4. 同步空文件夹开关（孤立叶子目录）
    if (
      !strategy.syncEmptyDirs &&
      row &&
      row.kind === 'dir' &&
      isLeafOrphanDir(row) &&
      (op.op === 'copyLeftToRight' ||
        op.op === 'copyRightToLeft' ||
        op.op === 'deleteLeft' ||
        op.op === 'deleteRight')
    ) {
      skipped.push({ ...op, reason: `${op.reason}; strategy:skipEmptyDir` });
      continue;
    }

    ops.push(op);
  }
  return { ops, skipped };
}

/** 计划预览行（状态机：预设动作 + 用户改写 + 策略跳过 的合成视图） */
export interface SyncPlanRow {
  /** 行 id（关联/改写键，**非文件地址**） */
  path: string;
  /** 展示用真实路径（计划预览显示它，与最终执行地址一致） */
  displayPath?: string;
  /** 执行载荷所需的真实侧路径（R3-03：透传，不得重新拼接） */
  sourcePath?: string;
  targetPath?: string;
  operationPath?: string;
  name: string;
  kind: DirRow['kind'];
  /** 最终动作（keep = 不动作） */
  effectiveOp: PlannedOpKind | 'keep';
  /** 动作来源 */
  source: 'auto' | 'override' | 'strategySkip' | 'userKeep';
  /** 删除警示（OP-001 确认体系：删除操作红色标记） */
  warning: boolean;
  /** 方向着色：l2r / r2l / none（预览表方向列） */
  direction: 'l2r' | 'r2l' | 'none';
  reason: string;
}

/** 逐项改写应用到预设计划（返回每行最终动作与来源标记） */
export function buildSyncPlanRows(
  ops: readonly PlannedOperation[],
  strategySkipped: readonly PlannedOperation[],
  rows: readonly DirRow[],
  overrides: Readonly<Record<string, RowOpOverride>>,
): SyncPlanRow[] {
  const byId = new Map(rows.map((r) => [r.id, r]));
  const opByPath = new Map<string, PlannedOperation>();
  for (const op of ops) opByPath.set(op.path, op);
  const skippedPaths = new Set(strategySkipped.map((p) => p.path));
  const out: SyncPlanRow[] = [];
  const seen = new Set<string>();
  // 1) 预设计划行（含策略跳过行）
  for (const op of ops) {
    seen.add(op.path);
    const row = byId.get(op.path);
    const override = overrides[op.path] ?? 'auto';
    const isStrategySkipped = skippedPaths.has(op.path);
    let effectiveOp: PlannedOpKind | 'keep' = op.op;
    let source: SyncPlanRow['source'] = 'auto';
    if (override === 'keep') {
      effectiveOp = 'keep';
      source = 'userKeep';
    } else if (override !== 'auto') {
      effectiveOp = override;
      source = 'override';
    } else if (isStrategySkipped) {
      effectiveOp = 'keep';
      source = 'strategySkip';
    }
    out.push(toPlanRow(op, row, effectiveOp, source, op.reason));
  }
  for (const op of strategySkipped) {
    if (seen.has(op.path)) continue;
    seen.add(op.path);
    const row = byId.get(op.path);
    const override = overrides[op.path] ?? 'auto';
    const effectiveOp = override === 'keep' ? 'keep' : override === 'auto' ? 'keep' : override;
    out.push(
      toPlanRow(op, row, effectiveOp, override === 'auto' ? 'strategySkip' : 'override', op.reason),
    );
  }
  return out;

  function toPlanRow(
    op: PlannedOperation,
    row: DirRow | undefined,
    effectiveOp: PlannedOpKind | 'keep',
    source: SyncPlanRow['source'],
    reason: string,
  ): SyncPlanRow {
    const path = op.path;
    return {
      path,
      displayPath: op.displayPath ?? op.targetPath ?? op.operationPath ?? op.sourcePath ?? op.path,
      sourcePath: op.sourcePath,
      targetPath: op.targetPath,
      operationPath: op.operationPath,
      name: row?.name ?? path.split('/').pop() ?? path,
      kind: row?.kind ?? 'file',
      effectiveOp,
      source,
      warning: effectiveOp === 'deleteLeft' || effectiveOp === 'deleteRight',
      direction:
        effectiveOp === 'copyLeftToRight'
          ? 'l2r'
          : effectiveOp === 'copyRightToLeft'
            ? 'r2l'
            : 'none',
      reason,
    };
  }
}

/** 预览行 -> 执行操作批次（跳过 keep；复用 dirOps.plannedToOp 的映射口径） */
export function planRowsToExecuteOps(planRows: readonly SyncPlanRow[]): DirExecuteOp[] {
  const out: DirExecuteOp[] = [];
  for (const pr of planRows) {
    if (pr.effectiveOp === 'keep') continue;
    out.push(
      plannedToOp({
        op: pr.effectiveOp,
        path: pr.path,
        reason: pr.reason,
        sourcePath: pr.sourcePath,
        targetPath: pr.targetPath,
        operationPath: pr.operationPath,
        displayPath: pr.displayPath,
      }),
    );
  }
  return out;
}

/** 计划统计（确认框/预览统计行） */
export interface SyncPlanStats {
  copyL2R: number;
  copyR2L: number;
  deleteL: number;
  deleteR: number;
  keep: number;
  total: number;
}

export function summarizeSyncPlan(planRows: readonly SyncPlanRow[]): SyncPlanStats {
  const s: SyncPlanStats = {
    copyL2R: 0,
    copyR2L: 0,
    deleteL: 0,
    deleteR: 0,
    keep: 0,
    total: planRows.length,
  };
  for (const pr of planRows) {
    if (pr.effectiveOp === 'keep') s.keep++;
    else if (pr.effectiveOp === 'copyLeftToRight') s.copyL2R++;
    else if (pr.effectiveOp === 'copyRightToLeft') s.copyR2L++;
    else if (pr.effectiveOp === 'deleteLeft') s.deleteL++;
    else if (pr.effectiveOp === 'deleteRight') s.deleteR++;
  }
  return s;
}

/** 同步结果报告（执行后页面 + 文本导出的数据面） */
export interface SyncExecReport {
  copied: number;
  deleted: number;
  touched: number;
  skipped: number;
  failed: number;
  total: number;
  cancelled: boolean;
  details: { kind: string; path: string; status: string; detail: string | null }[];
}

/** 由 dir/execute 结果聚合同步报告（dryRun 明细计为 skipped 展示） */
export function reportFromExecute(result: {
  total: number;
  succeeded: { op: DirExecuteOp; detail: string | null }[];
  failed: { op: DirExecuteOp; status: string; detail: string | null }[];
  skipped: { op: DirExecuteOp; status: string; detail: string | null }[];
  cancelled: boolean;
  dryRun: boolean;
}): SyncExecReport {
  const classify = (op: DirExecuteOp): 'copied' | 'deleted' | 'touched' => {
    if (op.kind === 'delete') return 'deleted';
    if (op.kind === 'touch') return 'touched';
    return 'copied';
  };
  const details = [
    ...result.failed.map((r) => ({
      kind: r.op.kind,
      path: r.op.path,
      status: 'failed',
      detail: r.detail,
    })),
    ...result.skipped.map((r) => ({
      kind: r.op.kind,
      path: r.op.path,
      status: 'skipped',
      detail: r.detail,
    })),
    ...result.succeeded.map((r) => ({
      kind: r.op.kind,
      path: r.op.path,
      status: classify(r.op),
      detail: r.detail,
    })),
  ];
  return {
    copied: result.succeeded.filter((r) => classify(r.op) === 'copied').length,
    deleted: result.succeeded.filter((r) => classify(r.op) === 'deleted').length,
    touched: result.succeeded.filter((r) => classify(r.op) === 'touched').length,
    skipped: result.skipped.length,
    failed: result.failed.length,
    total: result.total,
    cancelled: result.cancelled,
    details,
  };
}

/** 导出文本报告（FR-SYNC 报告/日志需求；剪贴板/下载共用） */
export function exportSyncReportText(
  meta: { preset: string; strategy: SyncStrategyUi; left: string; right: string; dryRun: boolean },
  report: SyncExecReport,
): string {
  const lines: string[] = [];
  lines.push(`ai-compare 文件夹同步报告${meta.dryRun ? '（试运行）' : ''}`);
  lines.push(`预设：${meta.preset}`);
  lines.push(`左侧：${meta.left}`);
  lines.push(`右侧：${meta.right}`);
  lines.push(
    `策略：删除孤立项=${meta.strategy.deleteOrphans ? '开' : '关'} / 时间戳冲突=${meta.strategy.leftWins ? '左侧优先' : '较新优先'} / 只读侧=${meta.strategy.readonlySide === 'none' ? '无' : meta.strategy.readonlySide} / 同步空文件夹=${meta.strategy.syncEmptyDirs ? '开' : '关'}`,
  );
  lines.push('');
  lines.push(
    `统计：共 ${report.total} 项 — 复制 ${report.copied} / 删除 ${report.deleted} / 对时 ${report.touched} / 跳过 ${report.skipped} / 失败 ${report.failed}${report.cancelled ? '（已取消）' : ''}`,
  );
  lines.push('');
  lines.push('明细：');
  for (const d of report.details) {
    lines.push(`  [${d.status}] ${d.kind} ${d.path}${d.detail ? ` — ${d.detail}` : ''}`);
  }
  return lines.join('\n');
}
