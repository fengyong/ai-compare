import { describe, expect, it } from 'vitest';
import {
  allCollapsibleKeys,
  computeFolds,
  defaultCollapsedKeys,
  equalRuns,
  foldRangeAt,
  hiddenRowCount,
  runCollapsible,
  unfoldKeysFor,
  DEFAULT_CONTEXT_ROWS,
} from './folding';
import type { AlignedRow } from './alignment';

/** 快速构造 rows：字符串 'e' = equal，'d' = 差异行（changed），'a' = absent */
function rowsOf(pattern: string): AlignedRow[] {
  let sid = 0;
  return [...pattern].map((ch) => ({
    leftLine: ch === 'a' ? undefined : 0,
    rightLine: 0,
    status: ch === 'e' ? 'equal' : ch === 'a' ? 'absent' : 'changed',
    sectionId: sid++,
  }));
}

describe('folding：等值区折叠与跨折叠导航（FR-TXT-020~023）', () => {
  it('equalRuns：最大化连续 equal 段', () => {
    const runs = equalRuns(rowsOf('eeeddeeee'));
    expect(runs.map((r) => [r.firstRow, r.lastRow])).toEqual([
      [0, 2],
      [5, 8],
    ]);
    expect(runs[0].key).toBe(0);
  });

  it('runCollapsible：长度 > 2×context+1 才可折叠（默认 context=3 → 至少 8 行）', () => {
    expect(runCollapsible({ firstRow: 0, lastRow: 6, key: 0 }, DEFAULT_CONTEXT_ROWS)).toBe(false); // 7 行
    expect(runCollapsible({ firstRow: 0, lastRow: 7, key: 0 }, DEFAULT_CONTEXT_ROWS)).toBe(true); // 8 行
    // 小上下文下更短的段也可折叠
    expect(runCollapsible({ firstRow: 0, lastRow: 4, key: 0 }, 1)).toBe(true);
  });

  it('computeFolds：隐藏区间保留两侧上下文', () => {
    const rows = rowsOf('e'.repeat(10));
    const keys = defaultCollapsedKeys(rows, 3);
    expect(keys.size).toBe(1);
    const folds = computeFolds(rows, 3, keys);
    expect(folds).toEqual([{ fromRow: 3, toRow: 6, runKey: 0 }]);
    expect(hiddenRowCount(folds)).toBe(4);
  });

  it('上下文行数可配：context=1 时折叠更深', () => {
    const rows = rowsOf('e'.repeat(6));
    const keys = defaultCollapsedKeys(rows, 1);
    const folds = computeFolds(rows, 1, keys);
    expect(folds).toEqual([{ fromRow: 1, toRow: 4, runKey: 0 }]);
  });

  it('差异段与短等值段不折叠', () => {
    const rows = rowsOf('eeeddeeee');
    const folds = computeFolds(rows, 3, defaultCollapsedKeys(rows, 3));
    // 第一段 3 行（不满足 8 行阈值）、第二段 4 行 → 均不折叠
    expect(folds).toEqual([]);
  });

  it('全部展开 / 全部折叠', () => {
    const rows = rowsOf('e'.repeat(10) + 'd' + 'e'.repeat(9));
    const all = allCollapsibleKeys(rows, 3);
    expect(all.size).toBe(2);
    expect(computeFolds(rows, 3, all)).toHaveLength(2);
    expect(computeFolds(rows, 3, new Set())).toEqual([]); // 全部展开
  });

  it('foldRangeAt / unfoldKeysFor：查找命中折叠区先展开（跨折叠）', () => {
    const rows = rowsOf('e'.repeat(10));
    const folds = computeFolds(rows, 3, defaultCollapsedKeys(rows, 3));
    expect(foldRangeAt(folds, 4)).toEqual({ fromRow: 3, toRow: 6, runKey: 0 });
    expect(foldRangeAt(folds, 2)).toBeNull(); // 上下文行可见
    expect(foldRangeAt(folds, 7)).toBeNull();
    expect(unfoldKeysFor(folds, [4])).toEqual(new Set([0]));
    expect(unfoldKeysFor(folds, [0, 9])).toEqual(new Set());
  });

  it('差异行永不落入折叠区间（导航目标始终可见）', () => {
    const rows = rowsOf('e'.repeat(10) + 'dd' + 'e'.repeat(9));
    const folds = computeFolds(rows, 3, defaultCollapsedKeys(rows, 3));
    for (const f of folds) {
      for (let r = f.fromRow; r <= f.toRow; r++) {
        expect(rows[r].status).toBe('equal');
      }
    }
  });

  it('hiddenRowCount：隐藏行计数（FR-TXT-003 计数条数据源）', () => {
    const rows = rowsOf('e'.repeat(10) + 'd' + 'e'.repeat(9));
    expect(hiddenRowCount(computeFolds(rows, 3, defaultCollapsedKeys(rows, 3)))).toBe(4 + 3);
  });
});
