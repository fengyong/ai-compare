/**
 * 等值区折叠模型（FR-TXT-020~023 折叠与导航，P5）。
 *
 * 折叠语义：等值行段（aligned 行中连续 equal 段）超出"上下文 2×N + 1"时，
 * 中段可折叠为占位条（"⋯ 已折叠 K 行 ⋯"）；差异行/上下文行永不折叠，
 * 因此差异导航天然"跨折叠"——跳转目标始终可见；查找命中折叠区时先展开。
 *
 * 折叠状态 = 已折叠段 key 集合（key = 段首对齐行号）；重 diff 后按新 rows
 * 重新计算折叠区间，key 仍命中的段保持折叠（编辑后折叠位置自动跟随）。
 */

import type { AlignedRow } from './alignment';

export interface EqualRun {
  /** 段首对齐行（0 基，含） */
  firstRow: number;
  /** 段尾对齐行（含） */
  lastRow: number;
  /** 折叠状态 key（= firstRow） */
  key: number;
}

/** 隐藏区间（对齐行，含两端）；fromRow = run.firstRow + context，toRow = run.lastRow - context */
export interface FoldRange {
  fromRow: number;
  toRow: number;
  runKey: number;
}

/** 默认上下文行数（差异两侧保留的等值行数，BC "显示上下文 N 行"） */
export const DEFAULT_CONTEXT_ROWS = 3;

/**
 * 全部等值段（最大化连续 equal 行段）。
 */
export function equalRuns(rows: AlignedRow[]): EqualRun[] {
  const runs: EqualRun[] = [];
  let start = -1;
  for (let i = 0; i <= rows.length; i++) {
    const isEq = i < rows.length && rows[i].status === 'equal';
    if (isEq && start < 0) start = i;
    if (!isEq && start >= 0) {
      runs.push({ firstRow: start, lastRow: i - 1, key: start });
      start = -1;
    }
  }
  return runs;
}

/** 段是否可折叠：长度超过 2×context + 1（折叠后仍保有上下文与至少一行中段） */
export function runCollapsible(run: EqualRun, contextRows: number): boolean {
  return run.lastRow - run.firstRow + 1 > 2 * contextRows + 1;
}

/** 默认折叠状态：全部可折叠段折叠（BC 显示过滤器"上下文 N 行"的默认形态） */
export function defaultCollapsedKeys(rows: AlignedRow[], contextRows: number): Set<number> {
  const keys = new Set<number>();
  for (const run of equalRuns(rows)) {
    if (runCollapsible(run, contextRows)) keys.add(run.key);
  }
  return keys;
}

/** 由折叠状态计算隐藏区间（含两端行） */
export function computeFolds(rows: AlignedRow[], contextRows: number, collapsedKeys: Set<number>): FoldRange[] {
  const folds: FoldRange[] = [];
  for (const run of equalRuns(rows)) {
    if (!collapsedKeys.has(run.key) || !runCollapsible(run, contextRows)) continue;
    folds.push({
      fromRow: run.firstRow + contextRows,
      toRow: run.lastRow - contextRows,
      runKey: run.key,
    });
  }
  return folds;
}

/** 行是否落在某个折叠区间内 */
export function foldRangeAt(folds: FoldRange[], row: number): FoldRange | null {
  for (const f of folds) {
    if (row >= f.fromRow && row <= f.toRow) return f;
  }
  return null;
}

/**
 * 展开使目标行可见（查找命中折叠区 / 概览条点击）：返回应移除的 runKey 集合。
 * 若目标行本就可见（含折叠区外的上下文行）返回空集。
 */
export function unfoldKeysFor(folds: FoldRange[], targetRows: number[]): Set<number> {
  const keys = new Set<number>();
  for (const row of targetRows) {
    const f = foldRangeAt(folds, row);
    if (f) keys.add(f.runKey);
  }
  return keys;
}

/** 全部可折叠段的 key（"全部折叠"按钮） */
export function allCollapsibleKeys(rows: AlignedRow[], contextRows: number): Set<number> {
  const keys = new Set<number>();
  for (const run of equalRuns(rows)) {
    if (runCollapsible(run, contextRows)) keys.add(run.key);
  }
  return keys;
}

/** 折叠区间内的行数合计（隐藏行计数条，FR-TXT-003 显示过滤行计数） */
export function hiddenRowCount(folds: FoldRange[]): number {
  let n = 0;
  for (const f of folds) n += f.toRow - f.fromRow + 1;
  return n;
}
