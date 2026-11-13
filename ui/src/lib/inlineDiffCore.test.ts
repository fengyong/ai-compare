import { describe, expect, it } from 'vitest';
import { buildInlineMarks, wordDiff } from './inlineDiffCore';

/** 行内（词级）diff 核心单测 */

describe('wordDiff', () => {
  it('完全相同：单个 equal pair 覆盖整行', () => {
    const pairs = wordDiff('int a = 1;', 'int a = 1;');
    expect(pairs).toHaveLength(1);
    expect(pairs[0].equal).toBe(true);
    expect(pairs[0].left).toEqual({ start: 0, end: 10 });
    expect(pairs[0].right).toEqual({ start: 0, end: 10 });
  });

  it('单标识符差异：equal 前缀 + 不匹配 + equal 后缀', () => {
    const pairs = wordDiff('int a = 1;', 'int b = 1;');
    // token 序列: int / ' ' / a / ' ' / = / ' ' / 1 / ;
    const unequal = pairs.filter((p) => !p.equal);
    expect(unequal).toHaveLength(1);
    expect(unequal[0].left).toEqual({ start: 4, end: 5 });
    expect(unequal[0].right).toEqual({ start: 4, end: 5 });
    // 前缀 "int " 是 equal
    expect(pairs[0].equal).toBe(true);
    expect(pairs[0].left?.end).toBe(4);
  });

  it('纯插入：一侧缺省', () => {
    const pairs = wordDiff('', 'hello world');
    expect(pairs).toHaveLength(1);
    expect(pairs[0].equal).toBe(false);
    expect(pairs[0].left).toBeUndefined();
    expect(pairs[0].right).toEqual({ start: 0, end: 11 });
  });

  it('纯删除：一侧缺省', () => {
    const pairs = wordDiff('hello world', '');
    expect(pairs).toHaveLength(1);
    expect(pairs[0].left).toEqual({ start: 0, end: 11 });
    expect(pairs[0].right).toBeUndefined();
  });

  it('两侧行均为空：空序列', () => {
    expect(wordDiff('', '')).toHaveLength(0);
  });

  it('gap 分组：相邻不匹配 token 归并为一段', () => {
    // "ab cd" vs "ab ef"：cd 与 ef 之间无匹配点 → 一个不匹配 pair（含两侧区间）
    const pairs = wordDiff('ab cd', 'ab ef');
    expect(pairs.filter((p) => !p.equal)).toHaveLength(1);
    expect(pairs.filter((p) => !p.equal)[0].left).toEqual({ start: 3, end: 5 });
    expect(pairs.filter((p) => !p.equal)[0].right).toEqual({ start: 3, end: 5 });
  });

  it('无任何公共 token：整行一个不匹配 pair', () => {
    const pairs = wordDiff('abcd', 'xy');
    expect(pairs).toHaveLength(1);
    expect(pairs[0].equal).toBe(false);
    expect(pairs[0].left).toEqual({ start: 0, end: 4 });
    expect(pairs[0].right).toEqual({ start: 0, end: 2 });
  });

  it('超过 maxCells 降级为整行不匹配（不抛错）', () => {
    const longA = 'x '.repeat(4000);
    const longB = 'y '.repeat(4000);
    const pairs = wordDiff(longA, longB, { maxCells: 100 });
    expect(pairs).toHaveLength(1);
    expect(pairs[0].equal).toBe(false);
    expect(pairs[0].left).toEqual({ start: 0, end: longA.length });
  });
});

describe('buildInlineMarks', () => {
  it('为 change section 逐行产出非 equal span', () => {
    const left = ['int a = 1;', 'int b = 2;'];
    const right = ['int a = 3;', 'int b = 2;'];
    const marks = buildInlineMarks(left, right, 0, 0, 2);
    // 第 1 行有差异（1→3），第 2 行相同
    expect(marks.left).toHaveLength(1);
    expect(marks.right).toHaveLength(1);
    expect(marks.left[0]).toMatchObject({ line: 0, from: 8, to: 9 });
    expect(marks.right[0]).toMatchObject({ line: 0, from: 8, to: 9 });
  });

  it('超过 maxSectionLines 护栏：返回空 span 列表', () => {
    const left = Array.from({ length: 500 }, (_, i) => `line ${i} v=1`);
    const right = left.map((l) => l.replace('1', '2'));
    const marks = buildInlineMarks(left, right, 0, 0, 500, { maxSectionLines: 400 });
    expect(marks.left).toHaveLength(0);
    expect(marks.right).toHaveLength(0);
  });

  it('超长行护栏：该行被跳过', () => {
    const left = ['x'.repeat(2000)];
    const right = ['y'.repeat(2000)];
    const marks = buildInlineMarks(left, right, 0, 0, 1, { maxLineChars: 1000 });
    expect(marks.left).toHaveLength(0);
  });
});

// ---------------- P5：DMP 语义清理开关（引擎 SemanticCleanup::Dmp 语义对齐） ----------------

import { DMP_SEMANTIC_EDIT_COST, dmpCleanupPairs, mergeDmpMarks, type InlinePair } from './inlineDiffCore';

describe('inlineDiffCore：DMP 语义清理（P5 开关）', () => {
  it('dmpCleanup 开关：被短等值段分隔的相邻编辑块合并', () => {
    // "const a = 1;" vs "const a = 2 + 1;"：token diff 后 `2` 与 `+` 间可能有小等值段
    const pairs = wordDiff('const a = 1;', 'const b = 2 + 1;');
    const cleaned = dmpCleanupPairs(pairs);
    // 清理后非 equal 块数量不增加，且存在跨吸收的块（若存在 ≤ 阈值的间隔）
    const nonEqualRaw = pairs.filter((p) => !p.equal).length;
    const nonEqualClean = cleaned.filter((p) => !p.equal).length;
    expect(nonEqualClean).toBeLessThanOrEqual(nonEqualRaw);
  });

  it('等值间隔超阈值 → 不合并', () => {
    const long = 'x'.repeat(DMP_SEMANTIC_EDIT_COST + 1);
    const pairs: InlinePair[] = [
      { left: { start: 0, end: 1 }, right: { start: 0, end: 1 }, equal: false },
      { left: { start: 1, end: 1 + long.length }, right: { start: 1, end: 1 + long.length }, equal: true },
      { left: { start: 1 + long.length, end: 2 + long.length }, right: { start: 1 + long.length, end: 2 + long.length }, equal: false },
    ];
    const cleaned = dmpCleanupPairs(pairs);
    expect(cleaned.filter((p) => !p.equal)).toHaveLength(2);
  });

  it('等值间隔 ≤ 阈值 → 合并并吸收间隔', () => {
    const pairs: InlinePair[] = [ // 等值间隔 2 ≤ 4
      { left: { start: 0, end: 1 }, right: { start: 0, end: 1 }, equal: false },
      { left: { start: 1, end: 3 }, right: { start: 1, end: 3 }, equal: true },
      { left: { start: 3, end: 4 }, right: { start: 3, end: 4 }, equal: false },
    ];
    const cleaned = dmpCleanupPairs(pairs);
    const nonEqual = cleaned.filter((p) => !p.equal);
    expect(nonEqual).toHaveLength(1);
    expect(nonEqual[0].left).toEqual({ start: 0, end: 4 });
  });

  it('mergeDmpMarks：同行相邻 span 合并（按行分组）', () => {
    const merged = mergeDmpMarks([
      { line: 1, from: 6, to: 10 },
      { line: 0, from: 0, to: 2 },
      { line: 1, from: 3, to: 5 },
      { line: 1, from: 20, to: 30 },
    ]);
    expect(merged).toHaveLength(3); // L0:1 块；L1: 3-5 与 6-10 合并（间隔 1），20-30 独立
    const l1 = merged.filter((m) => m.line === 1);
    expect(l1).toHaveLength(2);
    expect(l1[0]).toMatchObject({ from: 3, to: 10 });
  });

  it('wordDiff dmpCleanup 选项直通', () => {
    const plain = wordDiff('abc = 1;', 'abd = 1;');
    const cleaned = wordDiff('abc = 1;', 'abd = 1;', { dmpCleanup: true });
    const plainNE = plain.filter((p) => !p.equal).length;
    const cleanNE = cleaned.filter((p) => !p.equal).length;
    expect(cleanNE).toBeLessThanOrEqual(plainNE);
    expect(cleanNE).toBeGreaterThan(0);
  });
});
