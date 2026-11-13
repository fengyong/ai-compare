import { describe, expect, it } from 'vitest';
import {
  compileQuery,
  findMatches,
  findNext,
  matchedLineNos,
  matchesInLine,
  replaceAllInLine,
  expandReplacement,
  type FindQuery,
} from './findCore';

const q = (needle: string, extra: Partial<FindQuery> = {}): FindQuery => ({
  needle,
  regex: false,
  caseSensitive: false,
  wholeWord: false,
  ...extra,
});

describe('findCore：查找替换纯核心（FIND-001 / FR-TXT-022）', () => {
  it('文本查找（不区分大小写）：命中区间正确', () => {
    const c = compileQuery(q('abc'));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    const ms = matchesInLine('xxABCyyabc', 3, c);
    expect(ms).toEqual([
      { line: 3, from: 2, to: 5 },
      { line: 3, from: 7, to: 10 },
    ]);
  });

  it('区分大小写开关生效', () => {
    const c = compileQuery(q('abc', { caseSensitive: true }));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    expect(matchesInLine('xxABCyy', 0, c)).toEqual([]);
    expect(matchesInLine('xxabcyy', 0, c)).toEqual([{ line: 0, from: 2, to: 5 }]);
  });

  it('正则模式：分组与量词', () => {
    const c = compileQuery(q('\\d+', { regex: true }));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    const ms = matchesInLine('v12 = 345', 0, c);
    expect(ms).toEqual([
      { line: 0, from: 1, to: 3 },
      { line: 0, from: 6, to: 9 },
    ]);
  });

  it('非法正则返回错误信息而非抛异常', () => {
    const c = compileQuery(q('([unclosed', { regex: true }));
    expect(c.ok).toBe(false);
    if (!c.ok) expect(c.error.length).toBeGreaterThan(0);
  });

  it('全字匹配：cat 不命中 category / bobcat，命中独立 cat', () => {
    const c = compileQuery(q('cat', { wholeWord: true, caseSensitive: true }));
    expect(c.ok).toBe(true);
    expect(findMatches(['category', 'bob cat!', 'CAT'], c)).toEqual([{ line: 1, from: 4, to: 7 }]);
  });

  it('空 needle → 未输入态（ok=false 且 error 为空）', () => {
    const c = compileQuery(q(''));
    expect(c.ok).toBe(false);
    if (!c.ok) expect(c.error).toBe('');
  });

  it('findMatches：跨行计数 + 匹配计数', () => {
    const c = compileQuery(q('foo'));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    const lines = ['foo bar', 'baz', 'foo foo'];
    const ms = findMatches(lines, c);
    expect(ms).toHaveLength(3);
  });

  it('findNext：向后、当前位置不算、循环回绕', () => {
    const c = compileQuery(q('m'));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    const lines = ['m0', 'a', 'm2'];
    // 位置在第一个匹配上 → 下一个是 m2
    expect(findNext(lines, c, 0, 0, false)).toEqual({ line: 2, from: 0, to: 1 });
    // 尾部 → 回绕到第一个
    expect(findNext(lines, c, 2, 0, false)).toEqual({ line: 0, from: 0, to: 1 });
    // 向前
    expect(findNext(lines, c, 2, 0, true)).toEqual({ line: 0, from: 0, to: 1 });
    expect(findNext(lines, c, 0, 0, true)).toEqual({ line: 2, from: 0, to: 1 });
  });

  it('replaceAllInLine：文本模式全部替换', () => {
    const c = compileQuery(q('ab'));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    expect(replaceAllInLine('abxab', c, 'X', false)).toBe('XxX');
    expect(replaceAllInLine('abab', c, 'X', false)).toBe('XX');
    expect(replaceAllInLine('nohit', c, 'X', false)).toBeNull();
  });

  it('replaceAllInLine：正则模式 $1 组引用', () => {
    const c = compileQuery(q('(\\w+)=(\\d+)', { regex: true }));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    expect(replaceAllInLine('a=1 b=22', c, '$2:$1', true)).toBe('1:a 22:b');
  });

  it('expandReplacement：$& 与 $$', () => {
    expect(expandReplacement('cat', '[$&]', true, [])).toBe('[cat]');
    expect(expandReplacement('cat', '$$', true, [])).toBe('$');
    expect(expandReplacement('cat', 'X', false, [])).toBe('X');
  });

  it('matchedLineNos：全选匹配行', () => {
    const c = compileQuery(q('err'));
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    expect(matchedLineNos(['ok', 'error here', 'nothing', 'ERR'], c)).toEqual([1, 3]);
  });

  it('正则大小写开关对替换的影响', () => {
    const ci = compileQuery(q('err', { caseSensitive: false }));
    const cs = compileQuery(q('err', { caseSensitive: true }));
    expect(ci.ok && cs.ok).toBe(true);
    if (ci.ok && cs.ok) {
      expect(replaceAllInLine('ERR', ci, 'x', false)).toBe('x');
      expect(replaceAllInLine('ERR', cs, 'x', false)).toBeNull();
    }
  });
});
