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
