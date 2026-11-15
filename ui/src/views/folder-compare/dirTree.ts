/**
 * 目录树视图数据面（纯函数）：把引擎/缓存的行数组组织成树序可见行。
 *
 * 引擎 delta 的行序 = arena 创建序（父先于子，但兄弟目录的子树穿插），
 * 树形展示需要 DFS 序：本模块按 id 路径段构建父子索引，逐层排序
 * （目录先、名称次，大小写不敏感——BC 默认排序惯例），并按展开状态
 * 计算可见行。平铺模式直接按名称排序。
 */

import type { DirRow } from '../../protocol/types';

export interface TreeIndex {
  /** parentId（空串 = 根）-> 直接子行（已排序） */
  children: Map<string, DirRow[]>;
  /** id -> 行 */
  byId: Map<string, DirRow>;
}

/** P6b：表头排序键（点击列标题排序，FR-DIR-042；标记/选择以行 id 为键不受影响） */
export type DirSortKey = 'name' | 'leftSize' | 'rightSize' | 'leftMtime' | 'rightMtime';

export interface DirSortSpec {
  key: DirSortKey;
  /** 1 = 升序，-1 = 降序 */
  dir: 1 | -1;
}

/** 父 id（rel 路径的父目录；顶层行父为空串） */
export function parentOf(id: string): string {
  const i = id.lastIndexOf('/');
  return i === -1 ? '' : id.slice(0, i);
}

/** 排序键取值（缺侧 -> null：排序时沉底，与 BC 列排序惯例一致） */
function sortCell(row: DirRow, key: DirSortKey): string | number | null {
  switch (key) {
    case 'name':
      return row.name.toLowerCase();
    case 'leftSize':
      return row.left?.size ?? null;
    case 'rightSize':
      return row.right?.size ?? null;
    case 'leftMtime':
      return row.left?.mtimeMs ?? null;
    case 'rightMtime':
      return row.right?.mtimeMs ?? null;
  }
}

/** 逐层排序：目录先、名称次（大小写不敏感；同名目录在前稳定） */
function compareRows(a: DirRow, b: DirRow): number {
  const ad = a.kind === 'dir' ? 0 : 1;
  const bd = b.kind === 'dir' ? 0 : 1;
  if (ad !== bd) return ad - bd;
  const an = a.name.toLowerCase();
  const bn = b.name.toLowerCase();
  if (an !== bn) return an < bn ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/**
 * 表头列排序比较（目录仍排最前保持树形 DFS 父先于子；null 沉底；
 * 平手回退默认名称序保证稳定）。
 */
export function compareRowsBySpec(a: DirRow, b: DirRow, spec: DirSortSpec): number {
  const ad = a.kind === 'dir' ? 0 : 1;
  const bd = b.kind === 'dir' ? 0 : 1;
  if (ad !== bd) return ad - bd;
  const av = sortCell(a, spec.key);
  const bv = sortCell(b, spec.key);
  if (av === null && bv === null) return compareRows(a, b);
  if (av === null) return 1;
  if (bv === null) return -1;
  if (av !== bv) return (av < bv ? -1 : 1) * spec.dir;
  return compareRows(a, b);
}

export function buildTreeIndex(
  rows: readonly DirRow[],
  sort: DirSortSpec | null = null,
): TreeIndex {
  const children = new Map<string, DirRow[]>();
  const byId = new Map<string, DirRow>();
  for (const row of rows) {
    byId.set(row.id, row);
    const parent = parentOf(row.id);
    let list = children.get(parent);
    if (!list) {
      list = [];
      children.set(parent, list);
    }
    list.push(row);
  }
  const cmp = sort ? (a: DirRow, b: DirRow) => compareRowsBySpec(a, b, sort) : compareRows;
  for (const list of children.values()) list.sort(cmp);
  return { children, byId };
}

/**
 * 树序可见行：所有祖先已展开的行（DFS 序）；`hidden` 行不显示
 * （抑制模式下引擎发出 suppressed=true 的行仍显示，青绿色由视图层还原）。
 */
export function orderedTreeRows(index: TreeIndex, expanded: ReadonlySet<string>): DirRow[] {
  const out: DirRow[] = [];
  const walk = (parent: string): void => {
    const list = index.children.get(parent);
    if (!list) return;
    for (const row of list) {
      if (row.hidden) continue;
      out.push(row);
      if (row.kind === 'dir' && expanded.has(row.id)) {
        walk(row.id);
      }
    }
  };
  walk('');
  return out;
}

/** 平铺序可见行：全部非隐藏行按名称（或表头排序键）排序（kind dir 不参与） */
export function orderedFlatRows(
  rows: readonly DirRow[],
  sort: DirSortSpec | null = null,
): DirRow[] {
  return rows
    .filter((r) => !r.hidden && r.kind !== 'dir')
    .sort(sort ? (a, b) => compareRowsBySpec(a, b, sort) : compareRows);
}

/** 展开状态切换（返回新 Set；不可变） */
export function toggleExpanded(expanded: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(expanded);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  return next;
}

/** 展开全部目录 / 全部折叠（FR-DIR-050 编辑菜单命令） */
export function expandAll(rows: readonly DirRow[]): Set<string> {
  const next = new Set<string>();
  for (const row of rows) {
    if (row.kind === 'dir') next.add(row.id);
  }
  return next;
}

export const collapseAll = (): Set<string> => new Set();

/** 树缩进深度（行 id 路径段数） */
export function rowDepth(id: string): number {
  let depth = 0;
  for (const ch of id) {
    if (ch === '/') depth++;
  }
  return depth;
}
