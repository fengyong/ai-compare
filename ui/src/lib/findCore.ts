/**
 * 查找/替换纯函数核心（FR-TXT-022 / FIND-001，P5）。
 *
 * 与 CM6 解耦：输入为行数组（对齐文档行或单侧内容行），输出匹配区间
 * （行号 + 字符区间），由视图层换算成 CM6 装饰/选区/替换事务。
 * 支持开关：正则 / 区分大小写 / 全字匹配（FR-TXT-022 面板选项）。
 * 整文件匹配计数、循环查找（wrap）、正则替换（$1 组引用）齐备。
 */

export interface FindQuery {
  needle: string;
  regex: boolean;
  caseSensitive: boolean;
  wholeWord: boolean;
}

export interface TextMatch {
  line: number; // 0 基
  from: number;
  to: number;
}

/** 编译产物：re 带 g 标志供逐行扫描；wholeWord 由命中后边界校验落实 */
export type CompiledQuery =
  | { ok: true; re: RegExp; wholeWord: boolean }
  | { ok: false; error: string };

/** 编译查询：正则非法时返回错误（UI 提示，不抛异常）；空 needle 视为未输入 */
export function compileQuery(q: FindQuery): CompiledQuery {
  if (q.needle === '') {
    return { ok: false, error: '' };
  }
  let source: string;
  if (q.regex) {
    source = q.needle;
    try {
      void new RegExp(source);
    } catch (err) {
      return { ok: false, error: err instanceof Error ? err.message : String(err) };
    }
  } else {
    source = q.needle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
  const re = new RegExp(source, q.caseSensitive ? 'g' : 'gi');
  return { ok: true, re, wholeWord: q.wholeWord };
}

const WORD_CHAR = /[A-Za-z0-9_]/;

function isWordChar(ch: string | undefined): boolean {
  return ch !== undefined && ch !== '' && WORD_CHAR.test(ch);
}

/** 全字匹配边界校验：匹配两端不得紧邻单词字符 */
export function wholeWordOk(text: string, from: number, to: number): boolean {
  return !isWordChar(text[from - 1]) && !isWordChar(text[to]);
}

/** 单行内全部匹配（含整字边界过滤） */
export function matchesInLine(line: string, lineNo: number, compiled: CompiledQuery): TextMatch[] {
  if (!compiled.ok) return [];
  const out: TextMatch[] = [];
  compiled.re.lastIndex = 0;
  let m: RegExpExecArray | null;
  let guard = 0;
  while (guard++ < 10_000 && (m = compiled.re.exec(line)) !== null) {
    const from = m.index;
    const to = m.index + m[0].length;
    if (to > from && (!compiled.wholeWord || wholeWordOk(line, from, to))) {
      out.push({ line: lineNo, from, to });
    }
    if (m[0].length === 0) compiled.re.lastIndex++; // 空匹配防死循环
  }
  return out;
}

/** 在行集合中查找全部匹配（供计数与装饰构建） */
export function findMatches(lines: string[], compiled: CompiledQuery): TextMatch[] {
  if (!compiled.ok) return [];
  const out: TextMatch[] = [];
  for (let i = 0; i < lines.length; i++) {
    out.push(...matchesInLine(lines[i], i, compiled));
  }
  return out;
}

/**
 * 循环查找：从 (fromLine, fromCol) 起（当前位置上的匹配不算，FR-TXT-022）
 * 向后/向前找第一个匹配；到文件端回绕。找不到返回 null。
 */
export function findNext(
  lines: string[],
  compiled: CompiledQuery,
  fromLine: number,
  fromCol: number,
  backward: boolean
): TextMatch | null {
  if (!compiled.ok || lines.length === 0) return null;
  const all = findMatches(lines, compiled);
  if (all.length === 0) return null;
  if (!backward) {
    for (const m of all) {
      if (m.line > fromLine || (m.line === fromLine && m.from > fromCol)) return m;
    }
    return all[0]; // 回绕
  }
  for (let i = all.length - 1; i >= 0; i--) {
    const m = all[i];
    if (m.line < fromLine || (m.line === fromLine && m.from < fromCol)) return m;
  }
  return all[all.length - 1]; // 回绕
}

/** 生成单次替换文本（正则模式展开 $1..$99 / $& / $$；文本模式原样） */
export function expandReplacement(matchedText: string, replacement: string, regex: boolean, groups: string[]): string {
  if (!regex) return replacement;
  return replacement.replace(/\$(\d{1,2}|&|\$)/g, (_whole, token: string) => {
    if (token === '$') return '$';
    if (token === '&') return matchedText;
    const idx = Number(token);
    return groups[idx - 1] ?? ''; // 捕获组 1 基，数组 0 基
  });
}

/** 单行内全部替换（"全部替换"逐行应用）；返回 null 表示该行无匹配 */
export function replaceAllInLine(
  line: string,
  compiled: CompiledQuery,
  replacement: string,
  regex: boolean
): string | null {
  if (!compiled.ok) return null;
  const ms = matchesInLine(line, 0, compiled);
  if (ms.length === 0) return null;
  let out = '';
  let pos = 0;
  for (const m of ms) {
    const matchedText = line.slice(m.from, m.to);
    let groups: string[] = [];
    if (regex) {
      const re2 = new RegExp(compiled.re.source, compiled.re.flags.replace('g', ''));
      const gm = re2.exec(matchedText);
      groups = gm ? gm.slice(1).map((g) => g ?? '') : [];
    }
    out += line.slice(pos, m.from) + expandReplacement(matchedText, replacement, regex, groups);
    pos = m.to;
  }
  out += line.slice(pos);
  return out;
}

/** "全选匹配行"（FR-TXT-022）：返回包含至少一个匹配的行号（0 基）集合 */
export function matchedLineNos(lines: string[], compiled: CompiledQuery): number[] {
  if (!compiled.ok) return [];
  const out: number[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (matchesInLine(lines[i], i, compiled).length > 0) out.push(i);
  }
  return out;
}
