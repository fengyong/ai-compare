/**
 * 目录视图选择模型（FR-DIR-050 选择交互的 P6/P6b 子集，纯函数状态机）：
 * - 单击选择 / Ctrl+单击累加与反选 / Shift+单击范围连选（锚点区间）；
 * - 全选 / 反选 / 清空 / 按谓词选择（孤立、较新——编辑菜单命令的数据面）；
 * - 选择集合以行 id（rel 路径）为键，与 dir/operate 的 paths 参数同形；
 * - P6b：标记集（空格键标记，BC 键盘惯例 FR-DIR-050）纯函数 +
 *   编辑菜单命令选择器（较新的/较旧的/孤立项目，两侧=任一侧命中）。
 */

import type { DirRow } from '../../protocol/types';
import { isNewer, isOrphan } from './dirFilters';
import type { Side } from './dirOps';

export interface SelectionState {
  /** Shift 连选锚点（行 id；null = 尚未以普通单击建立） */
  anchor: string | null;
  /** 选中行 id 集合 */
  selected: ReadonlySet<string>;
}

export const emptySelection: SelectionState = { anchor: null, selected: new Set() };

export interface ClickModifiers {
  ctrl: boolean;
  shift: boolean;
}

/** 可见行 id 序（Shift 连选的区间依据；树形/平铺模式各自给出） */
export type VisibleIds = readonly string[];

/** 行点击 -> 新选择状态 */
export function onRowClick(state: SelectionState, rowId: string, mods: ClickModifiers, visible: VisibleIds): SelectionState {
  if (mods.shift) {
    const anchor = state.anchor ?? rowId;
    const start = visible.indexOf(anchor);
    const end = visible.indexOf(rowId);
    if (start === -1 || end === -1) {
      return { anchor, selected: new Set([rowId]) };
    }
    const [lo, hi] = start <= end ? [start, end] : [end, start];
    const selected = new Set<string>();
    for (let i = lo; i <= hi; i++) selected.add(visible[i]);
    return { anchor, selected };
  }
  if (mods.ctrl) {
    const selected = new Set(state.selected);
    if (selected.has(rowId)) {
      selected.delete(rowId);
    } else {
      selected.add(rowId);
    }
    return { anchor: rowId, selected };
  }
  // 普通单击：单选并重置锚点
  return { anchor: rowId, selected: new Set([rowId]) };
}

/** 全选 / 反选（基于当前可见行序） */
export function selectAll(visible: VisibleIds): SelectionState {
  return { anchor: null, selected: new Set(visible) };
}

export function invertSelection(state: SelectionState, visible: VisibleIds): SelectionState {
  const selected = new Set<string>();
  for (const id of visible) {
    if (!state.selected.has(id)) selected.add(id);
  }
  return { anchor: null, selected };
}

export function clearSelection(): SelectionState {
  return emptySelection;
}

/** 按谓词选择（编辑菜单"选择较新的/孤立项目"命令数据面；泛型允许完整 DirRow 谓词） */
export function selectWhere<T extends { id: string; visible: boolean }>(rows: readonly T[], pred: (row: T) => boolean): SelectionState {
  const selected = new Set<string>();
  for (const row of rows) {
    if (row.visible && pred(row)) selected.add(row.id);
  }
  return { anchor: null, selected };
}

/** 选择的可见行子集（保持 visible 顺序） */
export function selectedIdsInOrder(state: SelectionState, visible: VisibleIds): string[] {
  return visible.filter((id) => state.selected.has(id));
}

// ------------------------------------------------------------------
// 标记集（P6b：空格键标记；BC 惯例——定位到文件按空格键标记）
// ------------------------------------------------------------------

/** 标记集（行 id -> 是否标记；不可变更新） */
export type MarkSet = ReadonlySet<string>;

export const emptyMarks: MarkSet = new Set();

/** 切换单行标记 */
export function toggleMark(marked: MarkSet, id: string): Set<string> {
  const next = new Set(marked);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** 批量设置标记（选中行整批打标/取消——状态取多数行的反义?：以 value 显式指定） */
export function setMarks(marked: MarkSet, ids: readonly string[], value: boolean): Set<string> {
  const next = new Set(marked);
  for (const id of ids) {
    if (value) next.add(id);
    else next.delete(id);
  }
  return next;
}

/** 清空标记 */
export function clearMarks(): Set<string> {
  return new Set();
}

/** 行 id 是否被标记（缺省 false） */
export function isMarked(marked: MarkSet, id: string): boolean {
  return marked.has(id);
}

// ------------------------------------------------------------------
// 编辑菜单命令（§3.1.6 编辑菜单：选择较新的/较旧的/孤立项目 + 全选/反选）
// ------------------------------------------------------------------

/** 编辑菜单选择命令（「两侧」= 任一侧命中即选中该行） */
export type EditSelectCommand =
  | 'selectAll'
  | 'selectFiles'
  | 'invert'
  | 'selectNewerLeft'
  | 'selectNewerRight'
  | 'selectNewerBoth'
  | 'selectOlderLeft'
  | 'selectOlderRight'
  | 'selectOlderBoth'
  | 'selectOrphanLeft'
  | 'selectOrphanRight'
  | 'selectOrphanBoth';

/** 行谓词（较旧 = quickDiff 且对侧严格更新；与 dirFilters.isNewer 同口径） */
export function isOlder(row: DirRow, side: Side): boolean {
  const l = row.left?.mtimeMs ?? 0;
  const r = row.right?.mtimeMs ?? 0;
  return row.status === 'quickDiff' && (side === 'left' ? l < r : r < l);
}

/**
 * 编辑菜单命令 -> 新选择状态。
 * rows 须带 visible 标记（显示过滤/仅显示标记后的可见性；被隐藏行不参与）。
 */
export function runEditSelectCommand(
  cmd: EditSelectCommand,
  current: SelectionState,
  rows: readonly (DirRow & { visible: boolean })[],
  visible: VisibleIds
): SelectionState {
  switch (cmd) {
    case 'selectAll':
      return selectAll(visible);
    case 'invert':
      return invertSelection(current, visible);
    case 'selectFiles':
      return selectWhere(rows, (r) => r.kind === 'file');
    case 'selectNewerLeft':
    case 'selectNewerRight':
    case 'selectNewerBoth': {
      const side: Side | null = cmd === 'selectNewerLeft' ? 'left' : cmd === 'selectNewerRight' ? 'right' : null;
      return selectWhere(rows, (r) => (side === null ? isNewer(r, 'left') || isNewer(r, 'right') : isNewer(r, side)));
    }
    case 'selectOlderLeft':
    case 'selectOlderRight':
    case 'selectOlderBoth': {
      const side: Side | null = cmd === 'selectOlderLeft' ? 'left' : cmd === 'selectOlderRight' ? 'right' : null;
      return selectWhere(rows, (r) => (side === null ? isOlder(r, 'left') || isOlder(r, 'right') : isOlder(r, side)));
    }
    case 'selectOrphanLeft':
    case 'selectOrphanRight':
    case 'selectOrphanBoth': {
      const side: Side | null = cmd === 'selectOrphanLeft' ? 'left' : cmd === 'selectOrphanRight' ? 'right' : null;
      return selectWhere(rows, (r) => (side === null ? isOrphan(r.status, 'left') || isOrphan(r.status, 'right') : isOrphan(r.status, side)));
    }
  }
}
