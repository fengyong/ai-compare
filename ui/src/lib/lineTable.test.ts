import { describe, expect, it } from 'vitest';
import {
  buildDocText,
  docLinesOf,
  linesEqual,
  linesToEditText,
  parseDocText,
} from './lineTable';

/**
 * 行尾换算的权威口径（A1）：这是全 UI 唯一一份"行数组 ⇄ 文档文本"实现，
 * 单窗格编辑与双栏比较共用。换算错一行 → 引擎行数校验失败 → 重放 → 判分叉，
 * 最终表现是"插入的空行保存后消失"。
 */

/** 引擎侧解析口径（`LineTable::from_text`：结尾换行不产生新行） */
function engineParse(text: string): string[] {
  if (text === '') return [];
  const parts = text.split('\n');
  if (parts.length > 1 && parts[parts.length - 1] === '') parts.pop();
  return parts;
}

describe('lib/lineTable linesToEditText', () => {
  it('普通行：直接以 \\n 连接', () => {
    expect(linesToEditText([])).toBe('');
    expect(linesToEditText(['a'])).toBe('a');
    expect(linesToEditText(['a', 'b'])).toBe('a\nb');
  });

  it('末行为空：必须补行尾符（否则引擎少解析一行）', () => {
    // join 会吃掉结尾空段（`['a','']`.join = 'a\n'），必须再补一个行尾符才能让
    // 引擎解析出 2 行——这正是"末行为空"时必须补的理由
    expect(linesToEditText([''])).toBe('\n');
    expect(linesToEditText(['a', ''])).toBe('a\n\n');
    expect(linesToEditText(['a', 'b', ''])).toBe('a\nb\n\n');
  });

  it('多连续空行：行数与空行数逐个对上', () => {
    for (const count of [1, 2, 3, 5]) {
      const lines = Array.from({ length: count }, () => '');
      expect(engineParse(linesToEditText(lines))).toHaveLength(count);
    }
    expect(linesToEditText(['', ''])).toBe('\n\n');
    expect(linesToEditText(['a', '', ''])).toBe('a\n\n\n');
  });

  it('回归：直接 join 会丢末行（A1 的根因）', () => {
    // 旧实现（join）在"插入一个空行"时给出 ''，引擎解析成 0 行
    expect(engineParse([''].join('\n'))).toEqual([]);
    expect(engineParse(linesToEditText(['']))).toEqual(['']);
    // 中间空行 join 恰好正确，只有末行空行才会丢——这正是它长期没被发现的原因
    expect(engineParse(['a', '', 'b'].join('\n'))).toEqual(['a', '', 'b']);
  });

  it('与引擎解析互为逆运算（行数组 → 文本 → 行数组）', () => {
    const cases: string[][] = [
      [],
      ['a'],
      ['a', 'b'],
      ['a', 'b', ''],
      [''],
      ['', ''],
      ['', 'x', ''],
      ['a', '', 'b', ''],
      ['x', '', '', ''],
    ];
    for (const lines of cases) {
      expect(engineParse(linesToEditText(lines)), JSON.stringify(lines)).toEqual(lines);
    }
  });
});

describe('lib/lineTable docLinesOf', () => {
  it('空文本 0 行；其余按 \\n 拆分（末尾空行保留）', () => {
    expect(docLinesOf('')).toEqual([]);
    expect(docLinesOf('a')).toEqual(['a']);
    expect(docLinesOf('a\n')).toEqual(['a', '']);
    expect(docLinesOf('\n')).toEqual(['', '']);
  });

  it('空文本歧义：带 prev 时保留"唯一空行"，否则视为空文件', () => {
    expect(docLinesOf('', [''])).toEqual(['']);
    expect(docLinesOf('', ['a'])).toEqual([]);
    expect(docLinesOf('', [])).toEqual([]);
  });

  it('与 linesToEditText 双向一致（带 prev 消除歧义）', () => {
    const cases: string[][] = [[], ['a'], [''], ['a', ''], ['', ''], ['a', '', 'b']];
    for (const lines of cases) {
      expect(docLinesOf(lines.join('\n'), lines)).toEqual(lines);
    }
  });
});

describe('lib/lineTable parseDocText / buildDocText（A3 EOF 标记）', () => {
  it('空文件与"一个空行"必须区分开', () => {
    expect(parseDocText('')).toEqual({ lines: [], trailingEol: false });
    expect(parseDocText('\n')).toEqual({ lines: [''], trailingEol: true });
    expect(buildDocText([], false)).toBe('');
    expect(buildDocText([''], true)).toBe('\n');
  });

  it('同一个行数组 + 不同 EOF 标记 = 不同字节', () => {
    expect(buildDocText(['a'], true)).toBe('a\n');
    expect(buildDocText(['a'], false)).toBe('a');
    expect(buildDocText(['a', 'b'], true)).toBe('a\nb\n');
    expect(buildDocText(['a', 'b'], false)).toBe('a\nb');
  });

  it('互为逆运算（行数组 + 标记 → 文本 → 行数组 + 标记）', () => {
    const cases = [
      { lines: [] as string[], trailingEol: false },
      { lines: [''], trailingEol: true },
      { lines: ['a'], trailingEol: false },
      { lines: ['a'], trailingEol: true },
      { lines: ['a', ''], trailingEol: true },
      { lines: ['a', '', 'b'], trailingEol: false },
      { lines: ['', ''], trailingEol: true },
    ];
    for (const c of cases) {
      const label = JSON.stringify(c);
      expect(parseDocText(buildDocText(c.lines, c.trailingEol)), label).toEqual(c);
    }
  });

  it('与引擎解析同口径：文本里的行数不受 EOF 标记影响', () => {
    // "a\n" 与 "a" 都是 1 行——行数组看不出差别，这正是需要标记的原因
    expect(engineParse(buildDocText(['a'], true))).toEqual(['a']);
    expect(engineParse(buildDocText(['a'], false))).toEqual(['a']);
    expect(parseDocText('a\n').lines).toEqual(parseDocText('a').lines);
  });
});

describe('lib/lineTable linesEqual', () => {
  it('逐行比较，长度参与判定', () => {
    expect(linesEqual([], [])).toBe(true);
    expect(linesEqual(['a'], ['a'])).toBe(true);
    expect(linesEqual(['a'], ['a', ''])).toBe(false);
    expect(linesEqual(['a', 'b'], ['a', 'c'])).toBe(false);
    expect(linesEqual([''], [''])).toBe(true);
  });
});
