import { describe, expect, it } from 'vitest';
import { diffLinesOps, toSections, summarizeSections } from './lineDiffCore';

/** op 序列→简写（便于断言）：E=equal D=delete I=insert C=change */
function brief(a: string[], b: string[]) {
  return diffLinesOps(a, b).map((op) => {
    const aSeg = a.slice(op.aStart, op.aEnd).join('|');
    const bSeg = b.slice(op.bStart, op.bEnd).join('|');
    switch (op.type) {
      case 'equal':
        return `E[${aSeg}]`;
      case 'delete':
        return `D[${aSeg}]`;
      case 'insert':
        return `I[${bSeg}]`;
      case 'change':
        return `C[${aSeg}→${bSeg}]`;
    }
  });
}

describe('lineDiffCore：编辑重 diff 的客户端降级路径', () => {
  it('全等输入 → 单 equal op，无差异', () => {
    const lines = ['a', 'b', 'c'];
    expect(brief(lines, lines)).toEqual(['E[a|b|c]']);
  });

  it('公共前缀/后缀裁剪：中间单行修改 → 一个 change 块', () => {
    const a = ['header', 'old', 'footer'];
    const b = ['header', 'new', 'footer'];
    expect(brief(a, b)).toEqual(['E[header]', 'C[old→new]', 'E[footer]']);
  });

  it('纯插入 / 纯删除', () => {
    expect(brief(['a', 'c'], ['a', 'b', 'c'])).toEqual(['E[a]', 'I[b]', 'E[c]']);
    expect(brief(['a', 'b', 'c'], ['a', 'c'])).toEqual(['E[a]', 'D[b]', 'E[c]']);
  });

  it('编辑场景：等值区中改一行，前后等值区保持 equal', () => {
    const a = ['l0', 'l1', 'same', 'l3', 'l4'];
    const b = ['l0', 'L1-EDITED', 'same', 'l3', 'l4'];
    // 后缀裁剪把 same|l3|l4 归入尾部 equal
    expect(brief(a, b)).toEqual(['E[l0]', 'C[l1→L1-EDITED]', 'E[same|l3|l4]']);
  });

  it('Enter 拆行：一行变两行 → D+I（或 C）且行号衔接', () => {
    const a = ['ab', 'next'];
    const b = ['a', 'b-next'];
    const ops = diffLinesOps(a, b);
    // 结构断言：a 全消耗、b 全产出、区间连续
    expect(ops[0].aStart).toBe(0);
    expect(ops[ops.length - 1].aEnd).toBe(2);
    expect(ops[ops.length - 1].bEnd).toBe(2);
    const kinds = ops.map((o) => o.type);
    expect(kinds.some((k) => k !== 'equal')).toBe(true);
  });

  it('多 hunk 编辑：两处独立修改 → 两个 change 块', () => {
    const a = ['x0', 'A', 'mid', 'B', 'x4'];
    const b = ['x0', 'A2', 'mid', 'B2', 'x4'];
    const br = brief(a, b);
    expect(br.filter((s) => s.startsWith('C'))).toHaveLength(2);
    expect(br.filter((s) => s.startsWith('E'))).toHaveLength(3);
  });

  it('超限降级：n*m 超过 maxCells → 中段整体一个 change 块', () => {
    const a = Array.from({ length: 20 }, (_, i) => `a${i}`);
    const b = Array.from({ length: 20 }, (_, i) => `b${i}`);
    const ops = diffLinesOps(a, b, { maxCells: 10 });
    expect(ops).toHaveLength(1);
    expect(ops[0].type).toBe('change');
    expect(ops[0].aEnd - ops[0].aStart).toBe(20);
  });

  it('toSections：op → 协议 Section，id 连续、区间半开', () => {
    const a = ['h', 'old', 't'];
    const b = ['h', 'new', 't'];
    const sections = toSections(diffLinesOps(a, b));
    expect(sections.map((s) => s.id)).toEqual([0, 1, 2]);
    expect(sections.map((s) => s.kind)).toEqual(['equal', 'change', 'equal']);
    expect(sections[1].left).toEqual({ start: 1, end: 2 });
    expect(sections[1].right).toEqual({ start: 1, end: 2 });
    expect(sections[0].left).toEqual({ start: 0, end: 1 });
  });

  it('toSections：insert/delete 的空侧为 null', () => {
    const sections = toSections(diffLinesOps(['a'], ['a', 'b']));
    expect(sections.map((s) => s.kind)).toEqual(['equal', 'insert']);
    expect(sections[1].left).toBeNull();
    expect(sections[1].right).toEqual({ start: 1, end: 2 });
  });

  it('summarizeSections：统计口径与引擎 DiffSummary 一致', () => {
    const sections = toSections(diffLinesOps(['a', 'b', 'c'], ['a', 'B', 'c', 'd']));
    const s = summarizeSections(sections);
    expect(s.sectionCount).toBe(sections.length);
    expect(s.equalSections).toBe(2);
    expect(s.changeSections).toBe(1);
    expect(s.insertSections).toBe(1);
    expect(s.equalLines).toBe(2); // a、c
    expect(s.insertedLines).toBe(1); // d
  });

  it('空文件 vs 有内容', () => {
    expect(brief([], ['x'])).toEqual(['I[x]']);
    expect(brief(['x'], [])).toEqual(['D[x]']);
    expect(diffLinesOps([], [])).toEqual([]);
  });
});
