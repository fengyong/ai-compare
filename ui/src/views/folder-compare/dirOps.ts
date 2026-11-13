/**
 * 文件操作编排（FR-DIR-060~067 的 UI 侧数据面，纯函数）：
 * - 选中行 -> DirExecuteOp 批次（copy/move/rename/delete/touch/mkdir）；
 * - 文件夹语义（FR-DIR-060）：选中目录且其子项也在选择中时，忽略目录项
 *   仅处理子项；目录单独选中 = 整树操作（由引擎执行层递归落盘）；
 * - 命令汇总（确认框统计）、不可逆判定（OP-001 警示）、
 *   引擎 PlannedOperation 计划 -> DirExecuteOp 转换（镜像/更新预设链路）。
 */

import type { DirExecuteOp, DirRow, PlannedOperation, PlannedOpKind, SyncPreset } from '../../protocol/types';

export type { DirExecuteOp };

export type OpCommandKind = 'copyToRight' | 'copyToLeft' | 'moveToRight' | 'moveToLeft' | 'delete' | 'rename' | 'touch' | 'newFolder';

export type Side = 'left' | 'right';

export const otherSide = (s: Side): Side => (s === 'left' ? 'right' : 'left');

/** 目录行的子项是否也在选择中（FR-DIR-060 忽略文件夹语义） */
export function hasSelectedDescendant(rowId: string, selectedIds: ReadonlySet<string>): boolean {
  for (const id of selectedIds) {
    if (id !== rowId && id.startsWith(`${rowId}/`)) return true;
  }
  return false;
}

/**
 * 选中行 -> 操作批次。
 * 语义：
 * - copy/move 需要源侧存在（leftOnly 行不可复制到左侧——无源）；
 * - delete 默认删除两侧（BC 删除选定侧或两侧项目；此处 deleteBoth）；
 * - 目录项：若子项同被选中则跳过该目录项（子项覆盖）；否则整树一个 op。
 */
export function buildOpsForCommand(
  cmd: OpCommandKind,
  rows: readonly DirRow[],
  selectedIds: ReadonlySet<string>,
  arg?: { newName?: string; folderName?: string; parent?: string; side?: Side }
): DirExecuteOp[] {
  const byId = new Map(rows.map((r) => [r.id, r]));
  const ops: DirExecuteOp[] = [];
  const push = (op: DirExecuteOp) => ops.push(op);
  const targetIds = [...selectedIds].filter((id) => byId.has(id));

  switch (cmd) {
    case 'copyToRight':
    case 'copyToLeft':
    case 'moveToRight':
    case 'moveToLeft': {
      // 方向语义（FR-DIR-060）：复制/移动到右侧 = 取左源写右；到左侧反之
      const targetSide: Side = cmd.endsWith('Right') ? 'right' : 'left';
      const sourceSide: Side = targetSide === 'right' ? 'left' : 'right';
      const isMove = cmd.startsWith('move');
      for (const id of targetIds) {
        const row = byId.get(id)!;
        // 源必须存在：源侧孤立项不可复制
        if (!row[sourceSide]) continue;
        if (hasSelectedDescendant(id, selectedIds)) continue; // 子项覆盖（文件夹语义）
        push({
          kind: isMove ? 'move' : 'copy',
          path: id,
          side: sourceSide,
          targetSide,
        });
      }
      break;
    }
    case 'delete': {
      // 删除选定项的两侧（孤立项只删存在侧；目录整树递归）
      for (const id of targetIds) {
        const row = byId.get(id)!;
        if (row.kind === 'dir' && hasSelectedDescendant(id, selectedIds)) continue;
        if (row.left) {
          push({ kind: 'delete', path: id, side: 'left' });
        }
        if (row.right) {
          push({ kind: 'delete', path: id, side: 'right' });
        }
      }
      break;
    }
    case 'rename': {
      // 单选 + 单侧重命名（另一侧同名跟随由引擎 rename 语义覆盖——此处只重命名源侧）
      if (targetIds.length !== 1 || !arg?.newName) break;
      const row = byId.get(targetIds[0])!;
      const side: Side = arg.side ?? (row.left ? 'left' : 'right');
      push({ kind: 'rename', path: targetIds[0], side, newName: arg.newName });
      break;
    }
    case 'touch': {
      for (const id of targetIds) {
        const row = byId.get(id)!;
        if (row.left) push({ kind: 'touch', path: id, side: 'left', mtimeMs: 0 });
        if (row.right) push({ kind: 'touch', path: id, side: 'right', mtimeMs: 0 });
      }
      break;
    }
    case 'newFolder': {
      const parent = arg?.parent ?? '';
      const side = arg?.side ?? 'left';
      const name = arg?.folderName;
      if (!name) break;
      push({ kind: 'mkdir', path: parent ? `${parent}/${name}` : name, side });
      break;
    }
  }
  return ops;
}

/** 批次汇总（确认框统计行） */
export interface OpSummary {
  copies: number;
  moves: number;
  deletes: number;
  renames: number;
  touches: number;
  mkdirs: number;
  total: number;
}

export function summarizeOps(ops: DirExecuteOp[]): OpSummary {
  const s: OpSummary = { copies: 0, moves: 0, deletes: 0, renames: 0, touches: 0, mkdirs: 0, total: ops.length };
  for (const op of ops) {
    switch (op.kind) {
      case 'copy':
        s.copies++;
        break;
      case 'move':
        s.moves++;
        break;
      case 'delete':
        s.deletes++;
        break;
      case 'rename':
        s.renames++;
        break;
      case 'touch':
        s.touches++;
        break;
      case 'mkdir':
        s.mkdirs++;
        break;
    }
  }
  return s;
}

/** 是否含不可逆操作（OP-001 确认体系：删除/移动必须警示） */
export function hasIrreversible(ops: DirExecuteOp[]): boolean {
  return ops.some((op) => op.kind === 'delete' || op.kind === 'move');
}

// ------------------------------------------------------------------
// 引擎计划 -> 执行操作（同步预设链路）
// ------------------------------------------------------------------

/** PlannedOperation（dir/operate 计划）-> DirExecuteOp（dir/execute 载荷） */
export function opsFromPlan(plan: PlannedOperation[]): DirExecuteOp[] {
  return plan.map((p) => plannedToOp(p));
}

export function plannedToOp(p: PlannedOperation): DirExecuteOp {
  switch (p.op) {
    case 'copyLeftToRight':
      return { kind: 'copy', path: p.path, side: 'left', targetSide: 'right' };
    case 'copyRightToLeft':
      return { kind: 'copy', path: p.path, side: 'right', targetSide: 'left' };
    case 'deleteLeft':
      return { kind: 'delete', path: p.path, side: 'left' };
    case 'deleteRight':
      return { kind: 'delete', path: p.path, side: 'right' };
    case 'touchLeft':
      return { kind: 'touch', path: p.path, side: 'left', mtimeMs: 0 };
    case 'touchRight':
      return { kind: 'touch', path: p.path, side: 'right', mtimeMs: 0 };
  }
}

/** 计划统计（计划预览对话框）：按 op 种类 + 方向计数 */
export interface PlanSummary {
  copyLeftToRight: number;
  copyRightToLeft: number;
  deleteLeft: number;
  deleteRight: number;
  touchLeft: number;
  touchRight: number;
  total: number;
}

export function summarizePlan(plan: PlannedOperation[]): PlanSummary {
  const s: PlanSummary = { copyLeftToRight: 0, copyRightToLeft: 0, deleteLeft: 0, deleteRight: 0, touchLeft: 0, touchRight: 0, total: plan.length };
  for (const p of plan) {
    s[p.op as PlannedOpKind]++;
  }
  return s;
}

/** 预设 -> 展示名（计划预览标题） */
export const SYNC_PRESET_LABELS: Record<SyncPreset, string> = {
  mirrorLeftToRight: '镜像 左→右',
  mirrorRightToLeft: '镜像 右→左',
  updateLeftToRight: '更新 左→右',
  updateRightToLeft: '更新 右→左',
  updateBoth: '双向更新',
};

export const SYNC_PRESETS: SyncPreset[] = [
  'mirrorLeftToRight',
  'mirrorRightToLeft',
  'updateLeftToRight',
  'updateRightToLeft',
  'updateBoth',
];
