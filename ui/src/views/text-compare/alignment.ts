import type { Section, Side } from '../../protocol/types';
import { buildInlineMarks, type InlineMark } from '../../lib/inlineDiffCore';

/**
 * 对齐层核心（docs/03 §3.4.3 M32 的数据模型，风险 R10 的 P0 验证项）。
 *
 * 两个关键能力：
 * 1. buildAlignedDocs：把 section 序列展开成"行数相等、逐行 1:1 对齐"的左右两份文档。
 *    一侧缺行时插入填充行（空字符串 + absent 元数据），滚动与行号天然对齐；
 * 2. mapLineBySections：不经对齐文档、直接在 section 锚点上做行号映射
 *    （"按 section 对齐滚动映射"）。当未来进入换行模式（行高不再一致）时，
 *    同步滚动切换为锚点映射驱动，本函数即该路径的核心，P0 亦用于差异导航。
 */

export type AlignedRowStatus = 'equal' | 'inserted' | 'deleted' | 'changed' | 'absent';

/** 对齐文档中的一行元数据 */
export interface AlignedRow {
  /** 本行在左侧原文中的行号（0 基；填充行 undefined） */
  leftLine?: number;
  /** 本行在右侧原文中的行号（0 基；填充行 undefined） */
  rightLine?: number;
  status: AlignedRowStatus;
  sectionId: number;
}

export interface AlignedDoc {
  /** 文档文本行（含填充空行）；`lines.join('\n')` 即 CM6 doc */
  lines: string[];
  rows: AlignedRow[];
}

/**
 * 由 section 序列构建左右两份对齐文档。
 * - equal：n 行 1:1；
 * - insert（右独有）：左插填充行；
 * - delete（左独有）：右插填充行；
 * - change：max(l, r) 行，短侧缺行处填 absent（顶对齐，BC 惯例）。
 */
export function buildAlignedDocs(
  sections: Section[],
  leftLines: string[],
  rightLines: string[]
): { left: AlignedDoc; right: AlignedDoc } {
  const left: AlignedDoc = { lines: [], rows: [] };
  const right: AlignedDoc = { lines: [], rows: [] };

  const pushRows = (
    sectionId: number,
    kind: Section['kind'],
    lStart: number,
    lLen: number,
    rStart: number,
    rLen: number
  ): void => {
    const rowCount = Math.max(lLen, rLen);
    for (let k = 0; k < rowCount; k++) {
      const hasL = k < lLen;
      const hasR = k < rLen;
      const ll = hasL ? lStart + k : undefined;
      const rl = hasR ? rStart + k : undefined;

      let statusL: AlignedRowStatus;
      let statusR: AlignedRowStatus;
      if (kind === 'equal') {
        statusL = statusR = 'equal';
      } else if (kind === 'insert') {
        statusL = 'absent';
        statusR = 'inserted';
      } else if (kind === 'delete') {
        statusL = 'deleted';
        statusR = 'absent';
      } else {
        // change（含冲突）：有行的一侧 changed，缺行的一侧 absent
        statusL = hasL ? 'changed' : 'absent';
        statusR = hasR ? 'changed' : 'absent';
      }

      left.lines.push(ll !== undefined ? (leftLines[ll] ?? '') : '');
      right.lines.push(rl !== undefined ? (rightLines[rl] ?? '') : '');
      left.rows.push({ leftLine: ll, rightLine: rl, status: statusL, sectionId });
      right.rows.push({ leftLine: ll, rightLine: rl, status: statusR, sectionId });
    }
  };

  for (const s of sections) {
    const lLen = s.left ? s.left.end - s.left.start : 0;
    const rLen = s.right ? s.right.end - s.right.start : 0;
    if (lLen === 0 && rLen === 0) continue;
    pushRows(
      s.id,
      s.kind,
      s.left?.start ?? 0,
      lLen,
      s.right?.start ?? 0,
      rLen
    );
  }
  return { left, right };
}

/**
 * 按 section 锚点把 `side` 的行号映射到对侧行号。
 *
 * 规则：
 * - equal/change：区间内按偏移对应（change 顶对齐，偏移越界时取对侧区间末行）；
 * - 本侧独有（insert 的对面 / delete）：映射到对侧该 block 结束处之后的锚点（跟随滚动）；
 * - 差异块之间的间隙（相同区域）：按两块锚点线性外推；
 * - sections 为空：恒等映射（clamp 到对侧长度）。
 * 返回值始终 clamp 到 [0, otherLength-1]；otherLength <= 0 时返回 null。
 */
export function mapLineBySections(
  sections: Section[],
  side: Side,
  line: number,
  otherLength: number
): number | null {
  if (otherLength <= 0) return null;
  const myKey = side === 'left' ? ('left' as const) : ('right' as const);
  const otherKey = side === 'left' ? ('right' as const) : ('left' as const);
  const clamp = (v: number): number => Math.max(0, Math.min(v, otherLength - 1));
  if (sections.length === 0) return clamp(line);

  let myPos = 0;
  let otherPos = 0;

  for (const s of sections) {
    const mRange = s[myKey];
    const oRange = s[otherKey];

    // 1) 本 section 之前的相同间隙
    if (mRange && line < mRange.start) {
      return clamp(otherPos + (line - myPos));
    }
    // 2) 本 section 之内
    if (mRange && line < mRange.end) {
      if (oRange) {
        if (s.kind === 'equal') {
          return clamp(oRange.start + (line - mRange.start));
        }
        // change/conflict：顶对齐 + 偏移 clamp 到对侧区间
        const offset = Math.min(line - mRange.start, oRange.end - oRange.start - 1);
        return clamp(oRange.start + Math.max(0, offset));
      }
      // 本侧有、对侧无（delete 的本侧 / ……）：跟随到对侧当前推进位置
      return clamp(otherPos);
    }
    // 3) 推进游标（本侧独有段不消耗对侧游标，反之亦然）
    if (mRange) myPos = mRange.end;
    if (oRange) otherPos = oRange.end;
  }

  // 4) 尾部相同间隙
  return clamp(otherPos + (line - myPos));
}

/** 找出包含 `side` 行号 `line` 的差异 section（不含 equal）；无则返回 null */
export function findDiffSectionAtLine(
  sections: Section[],
  side: Side,
  line: number
): Section | null {
  for (const s of sections) {
    if (s.kind === 'equal') continue;
    const r = s[side];
    if (r && line >= r.start && line < r.end) return s;
  }
  return null;
}

/**
 * 为全部 change/conflict section 构建行内高亮 span（左右两侧）。
 * 规模护栏见 buildInlineMarks；P0 在文档构建时一次性计算（真实引擎可切换
 * 为 text/inlineDiff 可视区懒取，接口形态已对齐）。
 */
export function buildAllInlineMarks(
  sections: Section[],
  leftLines: string[],
  rightLines: string[]
): { left: InlineMark[]; right: InlineMark[] } {
  const left: InlineMark[] = [];
  const right: InlineMark[] = [];
  for (const s of sections) {
    if (s.kind !== 'change' && s.kind !== 'conflict') continue;
    if (!s.left || !s.right) continue;
    const lineCount = s.left.end - s.left.start;
    const marks = buildInlineMarks(leftLines, rightLines, s.left.start, s.right.start, lineCount);
    left.push(...marks.left);
    right.push(...marks.right);
  }
  return { left, right };
}
