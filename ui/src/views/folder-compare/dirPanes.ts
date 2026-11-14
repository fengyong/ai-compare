/**
 * BC 双窗格数据面（纯函数，FR-DIR-001/003/020/042/050）：
 * 左右窗格各自成列（名称/大小/已修改），中间 gutter 承载状态图标与复制箭头。
 * 行仍按名称对齐（一份 DirRow 两侧渲染），缺侧留白；选择与横向滚动按窗格独立，
 * 纵向滚动三列同步（否则对齐关系会在滚动时错位）。
 */

import type { DirRow } from '../../protocol/types';
import { S } from '../../strings';
import type { SelectionState } from './dirSelection';
import type { DirSortKey, DirSortSpec } from './dirTree';

export type PaneSide = 'left' | 'right';

export interface PaneColumn {
  label: string;
  /** null = 该列不参与排序 */
  key: DirSortKey | null;
  align: '' | 'r';
}

/** 单侧窗格列定义（排序键落到该侧的 size/mtime，名称列两侧共用同一键） */
export function paneColumns(side: PaneSide): PaneColumn[] {
  return [
    { label: S.folder.nameCol, key: 'name', align: '' },
    { label: S.folder.sizeCol, key: side === 'left' ? 'leftSize' : 'rightSize', align: 'r' },
    { label: S.folder.mtimeCol, key: side === 'left' ? 'leftMtime' : 'rightMtime', align: 'r' },
  ];
}

/** 该侧是否存在此项目（孤立项在缺侧整行留白） */
export function sidePresent(row: DirRow, side: PaneSide): boolean {
  return row[side] != null;
}

/** gutter 复制箭头可用性：源侧存在才允许复制到对侧 */
export function gutterCopyDirs(row: DirRow): { toLeft: boolean; toRight: boolean } {
  return { toLeft: sidePresent(row, 'right'), toRight: sidePresent(row, 'left') };
}

/** 表头排序循环：升序 → 降序 → 默认（FR-DIR-042；两侧表头共用一份排序规格） */
export function nextSortSpec(prev: DirSortSpec | null, key: DirSortKey): DirSortSpec | null {
  if (prev && prev.key === key) return prev.dir === 1 ? { key, dir: -1 } : null;
  return { key, dir: 1 };
}

/** 两侧独立选择集（FR-DIR-003/050：一侧的点击/全选不得影响另一侧） */
export type PaneSelections = Record<PaneSide, SelectionState>;

export function withPaneSelection(all: PaneSelections, side: PaneSide, next: SelectionState): PaneSelections {
  return { ...all, [side]: next };
}

export interface ScrollPos {
  id: string;
  top: number;
}

/**
 * 滚动同步计划：跳过源自身与已对齐者。
 * 少了任一条过滤都会形成 scroll 事件回环（赋值 → 事件 → 再赋值）。
 * 纵向（三列 scrollTop）与横向（窗格体 → 该侧表头 scrollLeft）共用。
 */
export function scrollSyncPlan(sourceId: string, sourceTop: number, others: readonly ScrollPos[]): ScrollPos[] {
  return others.filter((p) => p.id !== sourceId && p.top !== sourceTop).map((p) => ({ id: p.id, top: sourceTop }));
}