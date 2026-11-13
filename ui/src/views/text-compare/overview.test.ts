import { describe, expect, it } from 'vitest';
import {
  buildOverviewSegments,
  colorForRow,
  rowAtFraction,
  viewportFractions,
} from './overview';
import type { AlignedRow } from './alignment';

function rowsOf(pattern: string): AlignedRow[] {
  let sid = 0;
  return [...pattern].map((ch) => ({
    leftLine: ch === 'a' ? undefined : 0,
    rightLine: 0,
    status: ch === 'e' ? 'equal' : ch === 'a' ? 'absent' : 'changed',
    sectionId: sid++,
  }));
}

describe('overview：概览条模型（FR-TXT-030）', () => {
  it('colorForRow：状态×重要性 → 概览色（浅红优先于浅蓝语义）', () => {
    expect(colorForRow('equal', 'important')).toBe('equal');
    expect(colorForRow('absent', 'important')).toBe('absent');
    expect(colorForRow('changed', 'important')).toBe('diff');
    expect(colorForRow('inserted', 'unimportant')).toBe('unimportant');
    expect(colorForRow('deleted', 'ignored')).toBe('diff'); // ignored 视同 important 色带
  });

  it('buildOverviewSegments：游程编码合并相邻同色', () => {
    const imp = new Map<number, 'important' | 'unimportant' | 'ignored'>();
    // 3 equal + 2 diff + 3 equal
    const segs = buildOverviewSegments(rowsOf('eeecceeee'), imp);
    expect(segs).toEqual([
      { startRow: 0, rows: 3, color: 'equal' },
      { startRow: 3, rows: 2, color: 'diff' },
      { startRow: 5, rows: 4, color: 'equal' },
    ]);
  });

  it('buildOverviewSegments：不重要差异段独立成段', () => {
    const imp = new Map<number, 'important' | 'unimportant' | 'ignored'>([
      [2, 'unimportant'],
    ]);
    const segs = buildOverviewSegments(rowsOf('eec'), imp);
    expect(segs).toEqual([
      { startRow: 0, rows: 2, color: 'equal' },
      { startRow: 2, rows: 1, color: 'unimportant' },
    ]);
  });

  it('rowAtFraction：点击跳转行号映射（clamp）', () => {
    expect(rowAtFraction(100, 0)).toBe(0);
    expect(rowAtFraction(100, 0.5)).toBe(50);
    expect(rowAtFraction(100, 1)).toBe(99);
    expect(rowAtFraction(100, -3)).toBe(0);
    expect(rowAtFraction(0, 0.5)).toBe(0);
  });

  it('viewportFractions：视口矩形换算', () => {
    const r = viewportFractions(100, 10, 40)!;
    expect(r.top).toBeCloseTo(0.1);
    expect(r.height).toBeCloseTo(0.3);
    expect(viewportFractions(100, -5, 25)).toEqual({ top: 0, height: 0.25 });
    expect(viewportFractions(100, 50, 50)).toBeNull();
    expect(viewportFractions(0, 0, 0)).toBeNull();
  });
});
