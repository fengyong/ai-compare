/**
 * 概览条模型（FR-TXT-030 / docs/03 §3.4.3 缩略图总览条，P5）。
 *
 * mini-map 式 section 色带：对齐行按（状态, 重要性）游程编码为色段，
 * 画布按 totalRows 缩放绘制；视口矩形与当前差异段高亮叠加；点击/拖动跳转。
 * 纯函数可单测，画布渲染在 OverviewBar.tsx。
 */

import type { AlignedRow } from './alignment';

export type OverviewColor = 'equal' | 'diff' | 'unimportant' | 'absent';

/** 游程色段（startRow 起，连续 rows 行同色） */
export interface OverviewSeg {
  startRow: number;
  rows: number;
  color: OverviewColor;
}

export type Importance = 'important' | 'unimportant' | 'ignored';

/** 行状态 → 概览色 */
export function colorForRow(status: AlignedRow['status'], importance: Importance): OverviewColor {
  switch (status) {
    case 'equal':
      return 'equal';
    case 'absent':
      return 'absent';
    default:
      return importance === 'unimportant' ? 'unimportant' : 'diff';
  }
}

/** 游程编码（相邻同色段合并；rows 为空返回空数组） */
export function buildOverviewSegments(
  rows: AlignedRow[],
  importance: Map<number, Importance>
): OverviewSeg[] {
  const segs: OverviewSeg[] = [];
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    const imp = importance.get(row.sectionId) ?? 'important';
    const color = colorForRow(row.status, imp);
    const last = segs[segs.length - 1];
    if (last && last.color === color && last.startRow + last.rows === i) {
      last.rows++;
    } else {
      segs.push({ startRow: i, rows: 1, color });
    }
  }
  return segs;
}

/** 分数 → 行号（点击跳转；clamp 到有效范围） */
export function rowAtFraction(totalRows: number, frac: number): number {
  if (totalRows <= 0) return 0;
  const clamped = Math.max(0, Math.min(1, frac));
  return Math.min(totalRows - 1, Math.floor(clamped * totalRows));
}

/** 视口矩形（top/height 为 0..1 分数；空文档隐藏） */
export function viewportFractions(
  totalRows: number,
  topRow: number,
  bottomRow: number
): { top: number; height: number } | null {
  if (totalRows <= 0) return null;
  const top = Math.max(0, topRow) / totalRows;
  const bottom = Math.min(totalRows, bottomRow) / totalRows;
  if (bottom <= top) return null;
  return { top, height: bottom - top };
}

/** 概览条配色（CSS 变量名，palette 对齐附录 C 语义） */
export const OVERVIEW_COLORS: Record<OverviewColor, string> = {
  equal: 'var(--aic-overview-equal)',
  diff: 'var(--aic-overview-diff)',
  unimportant: 'var(--aic-overview-unimportant)',
  absent: 'var(--aic-overview-absent)',
};
