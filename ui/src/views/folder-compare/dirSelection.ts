/**
 * 目录视图选择模型（FR-DIR-050 选择交互的 P6 子集，纯函数状态机）：
 * - 单击选择 / Ctrl+单击累加与反选 / Shift+单击范围连选（锚点区间）；
 * - 全选 / 反选 / 清空 / 按谓词选择（孤立、较新——编辑菜单命令的数据面）；
 * - 选择集合以行 id（rel 路径）为键，与 dir/operate 的 paths 参数同形。
 */

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

/** 按谓词选择（编辑菜单"选择较新的/孤立项目"命令数据面） */
export function selectWhere(rows: { id: string; visible: boolean }[], pred: (row: { id: string }) => boolean): SelectionState {
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
