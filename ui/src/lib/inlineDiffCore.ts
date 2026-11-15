/**
 * 行内（词级）差异——纯函数核心，可单测。
 *
 * P0 策略：引擎的 `text/inlineDiff`（docs/03 §4.1 InlineResult，懒计算）是权威来源；
 * 该模块作为客户端兜底/演示路径（引擎未实现 inlineDiff 或演示引擎场景），
 * 在文本视图构建装饰时对 change section 计算 span 级高亮。
 *
 * 算法：token 化（词/空白/单字符）+ LCS DP，带 n*m 规模上限防爆炸
 * （超限降级为整行不匹配，对齐 docs/03 §6.2 的降级策略语义）。
 */

export interface CharRange {
  start: number;
  end: number;
}

export interface InlinePair {
  left?: CharRange;
  right?: CharRange;
  equal: boolean;
}

export interface InlineDiffOptions {
  /** LCS DP 单元格上限（n*m），超过则整行降级为单个不匹配 pair */
  maxCells?: number;
  /** DMP 语义清理（P5 UI 开关；与引擎 ai-core SemanticCleanup::Dmp 简化语义对齐） */
  dmpCleanup?: boolean;
}

/**
 * DMP 语义清理的合并阈值：相邻编辑区间被 ≤ 该字符数的等值段分隔时合并
 * （引擎 ai-core DMP_SEMANTIC_EDIT_COST 简化对应）。
 */
export const DMP_SEMANTIC_EDIT_COST = 4;

interface Token {
  text: string;
  start: number;
}

const TOKEN_RE = /\s+|[A-Za-z0-9_]+|./g;

function tokenize(line: string): Token[] {
  const tokens: Token[] = [];
  for (const m of line.matchAll(TOKEN_RE)) {
    tokens.push({ text: m[0], start: m.index });
  }
  return tokens;
}

/** 计算两行的词级差异 pair 序列（按出现顺序；equal 与非 equal 交替） */
export function wordDiff(left: string, right: string, opts: InlineDiffOptions = {}): InlinePair[] {
  const maxCells = opts.maxCells ?? 400_000;
  const a = tokenize(left);
  const b = tokenize(right);
  const n = a.length;
  const m = b.length;

  if (n === 0 && m === 0) return [];
  if (n === 0) return [{ right: { start: 0, end: right.length }, equal: false }];
  if (m === 0) return [{ left: { start: 0, end: left.length }, equal: false }];

  if (n * m > maxCells) {
    // 降级：整行不匹配（避免 O(n*m) 爆炸；超长行场景，docs/03 §6.2）
    return [
      {
        left: { start: 0, end: left.length },
        right: { start: 0, end: right.length },
        equal: false,
      },
    ];
  }

  // LCS 全表：dp[i][j] = LCS(a[0..i), b[0..j))
  const dp = new Uint32Array((n + 1) * (m + 1));
  const at = (i: number, j: number) => i * (m + 1) + j;
  for (let i = 1; i <= n; i++) {
    for (let j = 1; j <= m; j++) {
      dp[at(i, j)] =
        a[i - 1].text === b[j - 1].text
          ? dp[at(i - 1, j - 1)] + 1
          : Math.max(dp[at(i - 1, j)], dp[at(i, j - 1)]);
    }
  }

  // 回溯匹配点（token 序号对，双方递增），再按匹配点之间的 gap 分组为不匹配 pair：
  // 单 token 替换（如 a↔b）两侧各被匹配点包围，gap 恰好只含该 token，高亮最小化。
  const matchI: number[] = [];
  const matchJ: number[] = [];
  let i = n;
  let j = m;
  while (i > 0 && j > 0) {
    if (a[i - 1].text === b[j - 1].text) {
      matchI.push(i - 1);
      matchJ.push(j - 1);
      i--;
      j--;
    } else if (dp[at(i - 1, j)] >= dp[at(i, j - 1)]) {
      i--;
    } else {
      j--;
    }
  }
  matchI.reverse();
  matchJ.reverse();

  const tokenEnd = (tokens: Token[], idx: number, lineLen: number): number =>
    idx + 1 < tokens.length ? tokens[idx + 1].start : lineLen;

  const pairs: InlinePair[] = [];
  const push = (pair: InlinePair): void => {
    const last = pairs[pairs.length - 1];
    if (last && last.equal === pair.equal) {
      // 合并相邻同类区间（gap 分组下不会出现，保险起见保留）
      if (last.left && pair.left) last.left.end = pair.left.end;
      else if (!last.left && pair.left) last.left = pair.left;
      if (last.right && pair.right) last.right.end = pair.right.end;
      else if (!last.right && pair.right) last.right = pair.right;
    } else {
      pairs.push(pair);
    }
  };

  let prevI = 0;
  let prevJ = 0;
  for (let k = 0; k <= matchI.length; k++) {
    if (k === matchI.length) {
      // 尾部 gap：[prevI, n) 对 [prevJ, m)
      if (prevI < n || prevJ < m) {
        push({
          left:
            prevI < n ? { start: a[prevI].start, end: tokenEnd(a, n - 1, left.length) } : undefined,
          right:
            prevJ < m
              ? { start: b[prevJ].start, end: tokenEnd(b, m - 1, right.length) }
              : undefined,
          equal: false,
        });
      }
      break;
    }
    const mi = matchI[k];
    const mj = matchJ[k];
    // 匹配点之前的 gap
    if (mi > prevI || mj > prevJ) {
      push({
        left:
          mi > prevI ? { start: a[prevI].start, end: tokenEnd(a, mi - 1, left.length) } : undefined,
        right:
          mj > prevJ
            ? { start: b[prevJ].start, end: tokenEnd(b, mj - 1, right.length) }
            : undefined,
        equal: false,
      });
    }
    // 匹配 token 本身
    push({
      left: { start: a[mi].start, end: tokenEnd(a, mi, left.length) },
      right: { start: b[mj].start, end: tokenEnd(b, mj, right.length) },
      equal: true,
    });
    prevI = mi + 1;
    prevJ = mj + 1;
  }

  return opts.dmpCleanup ? dmpCleanupPairs(pairs) : pairs;
}

/**
 * DMP 语义清理（P5 UI 后处理开关；引擎 ai-core SemanticCleanup::Dmp 简化语义）：
 * 被 ≤ DMP_SEMANTIC_EDIT_COST 字符的等值段分隔的相邻编辑区间合并为一个编辑块，
 * 使高亮边界符合"语义整体"直觉（如 `a = 1` 与 `a = 2 + 1` 中相邻小改动连片）。
 */
export function dmpCleanupPairs(pairs: InlinePair[]): InlinePair[] {
  const out: InlinePair[] = [];
  const copyOf = (p: InlinePair): InlinePair => ({
    ...p,
    left: p.left ? { ...p.left } : undefined,
    right: p.right ? { ...p.right } : undefined,
  });
  for (const p of pairs) {
    if (p.equal) {
      out.push(copyOf(p));
      continue;
    }
    // 编辑块：回看 [编辑A, 短等值段] → 合并为一个编辑块（吸收等值间隔）
    if (out.length >= 2) {
      const eq = out[out.length - 1];
      const prevEdit = out[out.length - 2];
      const gapLen = eq.left
        ? eq.left.end - eq.left.start
        : eq.right
          ? eq.right.end - eq.right.start
          : Infinity;
      if (
        eq.equal &&
        !prevEdit.equal &&
        gapLen <= DMP_SEMANTIC_EDIT_COST &&
        prevEdit.left &&
        p.left
      ) {
        prevEdit.left.end = p.left.end;
        if (prevEdit.right && p.right) prevEdit.right.end = p.right.end;
        else if (p.right) prevEdit.right = p.right;
        out.pop(); // 吸收中间等值段
        continue;
      }
    }
    out.push(copyOf(p));
  }
  return out;
}

/** 行内高亮 span（某侧某行的字符区间，equal 区间不产出） */
export interface InlineMark {
  /** 0 基行号（对齐文档中的行） */
  line: number;
  from: number;
  to: number;
}

/** 同行内相邻高亮 span 合并（DMP 清理：等值间隔 ≤ 阈值时吸收为一块） */
export function mergeDmpMarks(marks: InlineMark[]): InlineMark[] {
  if (marks.length === 0) return marks;
  const byLine = new Map<number, InlineMark[]>();
  for (const m of marks) {
    const arr = byLine.get(m.line);
    if (arr) arr.push(m);
    else byLine.set(m.line, [m]);
  }
  const out: InlineMark[] = [];
  for (const arr of byLine.values()) {
    arr.sort((x, y) => x.from - y.from);
    let cur: InlineMark | null = null;
    for (const m of arr) {
      if (cur && m.from - cur.to <= DMP_SEMANTIC_EDIT_COST) {
        cur.to = Math.max(cur.to, m.to);
      } else {
        cur = { ...m };
        out.push(cur);
      }
    }
  }
  return out;
}

/**
 * 对一个 change section 的每一行对计算行内高亮 span。
 * 返回左右两侧各自的 mark 列表。带规模护栏：
 * - 行数 > maxSectionLines 或任一行 > maxLineChars 时跳过（仅保留行级底纹）。
 */
export function buildInlineMarks(
  leftLines: string[],
  rightLines: string[],
  leftStart: number,
  rightStart: number,
  lineCount: number,
  limits: {
    maxSectionLines?: number;
    maxLineChars?: number;
    maxCells?: number;
    dmpCleanup?: boolean;
  } = {},
): { left: InlineMark[]; right: InlineMark[] } {
  const maxSectionLines = limits.maxSectionLines ?? 400;
  const maxLineChars = limits.maxLineChars ?? 1000;
  const leftMarks: InlineMark[] = [];
  const rightMarks: InlineMark[] = [];
  if (lineCount > maxSectionLines) return { left: leftMarks, right: rightMarks };

  for (let k = 0; k < lineCount; k++) {
    const l = leftLines[leftStart + k] ?? '';
    const r = rightLines[rightStart + k] ?? '';
    if (l.length > maxLineChars || r.length > maxLineChars) continue;
    const pairs = wordDiff(l, r, { maxCells: limits.maxCells });
    for (const p of pairs) {
      if (p.equal) continue;
      if (p.left && p.left.end > p.left.start) {
        leftMarks.push({ line: leftStart + k, from: p.left.start, to: p.left.end });
      }
      if (p.right && p.right.end > p.right.start) {
        rightMarks.push({ line: rightStart + k, from: p.right.start, to: p.right.end });
      }
    }
  }
  return limits.dmpCleanup
    ? { left: mergeDmpMarks(leftMarks), right: mergeDmpMarks(rightMarks) }
    : { left: leftMarks, right: rightMarks };
}
