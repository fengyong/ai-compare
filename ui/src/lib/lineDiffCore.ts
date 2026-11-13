/**
 * 行级 diff 纯函数核心——客户端重 diff（P5 编辑实时重 diff 的 mock/降级路径）。
 *
 * 定位（任务书交付 1）：P3 协议面就绪前，编辑重 diff 在 UI 侧全量重算；
 * P3 开放 text/edit + 增量事件后由视图层按能力位切换，本模块保留为
 * 客户端兜底与小文件的快速路径。与引擎 ai-core 的 Myers 语义对齐处：
 * - 相同前缀/后缀先剔除（同 incremental.rs 窗口锚定思路）；
 * - 中段 LCS（Myers DP 简化实现）带单元格上限，超限降级为单一 change 块
 *   （docs/03 §6.2 降级策略语义）。
 *
 * 输出 op 序列同时服务两个消费方：
 * - toSections()：生成协议 Section[]（重新驱动对齐文档/概览条/导航）；
 * - op 游标：editPatch.ts 用它把"对齐文档的前后文本"换算成单侧内容补丁。
 */

import type { Section } from '../protocol/generated/Section';
import type { DiffSummary } from '../protocol/generated/DiffSummary';

export type DiffOpType = 'equal' | 'insert' | 'delete' | 'change';

/** 行级差异块；区间均为半开 [start, end)，0 基。change 同时持有两侧区间。 */
export interface DiffOp {
  type: DiffOpType;
  /** a 序列（旧/左）区间；insert 时 aStart === aEnd === 前缀推进位置 */
  aStart: number;
  aEnd: number;
  /** b 序列（新/右）区间；delete 时 bStart === bEnd === 前缀推进位置 */
  bStart: number;
  bEnd: number;
}

export interface LineDiffOptions {
  /** 中段 LCS 的 n*m 单元格上限；超过则中段降级为单个 change 块 */
  maxCells?: number;
}

/**
 * 行级 diff：先裁公共前缀/后缀，再对中段跑 LCS（含单元格护栏），
 * 输出按序排列、区间无缝衔接的 op 序列。
 */
export function diffLinesOps(a: string[], b: string[], opts: LineDiffOptions = {}): DiffOp[] {
  const maxCells = opts.maxCells ?? 2_000_000;
  const ops: DiffOp[] = [];
  const n = a.length;
  const m = b.length;

  // 公共前缀
  let pre = 0;
  while (pre < n && pre < m && a[pre] === b[pre]) pre++;
  if (pre > 0) ops.push({ type: 'equal', aStart: 0, aEnd: pre, bStart: 0, bEnd: pre });

  // 公共后缀（不与前缀重叠）
  let suf = 0;
  while (suf < n - pre && suf < m - pre && a[n - 1 - suf] === b[m - 1 - suf]) suf++;
  const midAEnd = n - suf;
  const midBEnd = m - suf;

  if (midAEnd > pre || midBEnd > pre) {
    for (const op of diffMiddle(a, b, pre, midAEnd, pre, midBEnd, maxCells)) ops.push(op);
  }

  if (suf > 0) {
    ops.push({
      type: 'equal',
      aStart: midAEnd,
      aEnd: n,
      bStart: midBEnd,
      bEnd: m,
    });
  }
  return ops;
}

/** 中段 LCS（空段或超限降级为 change 块）；输出 delete/insert/change 序列 */
function diffMiddle(
  a: string[],
  b: string[],
  aStart: number,
  aEnd: number,
  bStart: number,
  bEnd: number,
  maxCells: number
): DiffOp[] {
  const aLen = aEnd - aStart;
  const bLen = bEnd - bStart;
  if (aLen === 0 && bLen === 0) return [];
  if (aLen === 0) return [{ type: 'insert', aStart, aEnd, bStart, bEnd }];
  if (bLen === 0) return [{ type: 'delete', aStart, aEnd, bStart, bEnd }];
  if (aLen * bLen > maxCells) {
    // 护栏降级：中段整体一个 change 块（不再细分）
    return [{ type: 'change', aStart, aEnd, bStart, bEnd }];
  }

  // LCS 全表
  const dp = new Uint32Array((aLen + 1) * (bLen + 1));
  const at = (i: number, j: number) => i * (bLen + 1) + j;
  for (let i = 1; i <= aLen; i++) {
    const av = a[aStart + i - 1];
    for (let j = 1; j <= bLen; j++) {
      dp[at(i, j)] =
        av === b[bStart + j - 1]
          ? dp[at(i - 1, j - 1)] + 1
          : Math.max(dp[at(i - 1, j)], dp[at(i, j - 1)]);
    }
  }

  // 回溯出匹配点序列（i,j 对，双方递增）
  const matchI: number[] = [];
  const matchJ: number[] = [];
  let i = aLen;
  let j = bLen;
  while (i > 0 && j > 0) {
    if (a[aStart + i - 1] === b[bStart + j - 1]) {
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

  // 匹配点之间的 gap → change 块（相邻 delete+insert 合并，BC 语义）
  const ops: DiffOp[] = [];
  let pi = 0;
  let pj = 0;
  const flushGap = (gi: number, gj: number): void => {
    if (gi > pi && gj > pj) {
      ops.push({
        type: 'change',
        aStart: aStart + pi,
        aEnd: aStart + gi,
        bStart: bStart + pj,
        bEnd: bStart + gj,
      });
    } else if (gi > pi) {
      ops.push({ type: 'delete', aStart: aStart + pi, aEnd: aStart + gi, bStart: bStart + gj, bEnd: bStart + gj });
    } else if (gj > pj) {
      ops.push({ type: 'insert', aStart: aStart + gi, aEnd: aStart + gi, bStart: bStart + pj, bEnd: bStart + gj });
    }
  };
  for (let k = 0; k <= matchI.length; k++) {
    if (k === matchI.length) {
      flushGap(aLen, bLen);
      break;
    }
    flushGap(matchI[k], matchJ[k]);
    // 匹配行本身
    ops.push({
      type: 'equal',
      aStart: aStart + matchI[k],
      aEnd: aStart + matchI[k] + 1,
      bStart: bStart + matchJ[k],
      bEnd: bStart + matchJ[k] + 1,
    });
    pi = matchI[k] + 1;
    pj = matchJ[k] + 1;
  }
  return ops;
}

/** op 序列 → 协议 Section[]（重新驱动对齐文档；importance 恒 important，inline 未算） */
export function toSections(ops: DiffOp[]): Section[] {
  const sections: Section[] = [];
  let id = 0;
  for (const op of ops) {
    if (op.type === 'equal') {
      if (op.aEnd <= op.aStart) continue;
      sections.push({
        id: id++,
        kind: 'equal',
        left: { start: op.aStart, end: op.aEnd },
        right: { start: op.bStart, end: op.bEnd },
        important: 'important',
        inline: 'notComputed',
      });
    } else if (op.type === 'insert') {
      sections.push({
        id: id++,
        kind: 'insert',
        left: null,
        right: { start: op.bStart, end: op.bEnd },
        important: 'important',
        inline: 'notComputed',
      });
    } else if (op.type === 'delete') {
      sections.push({
        id: id++,
        kind: 'delete',
        left: { start: op.aStart, end: op.aEnd },
        right: null,
        important: 'important',
        inline: 'notComputed',
      });
    } else {
      sections.push({
        id: id++,
        kind: 'change',
        left: { start: op.aStart, end: op.aEnd },
        right: { start: op.bStart, end: op.bEnd },
        important: 'important',
        inline: 'notComputed',
      });
    }
  }
  return sections;
}

/** diff 统计（与引擎 DiffSummary 口径一致；mockEngine.summarize 同构） */
export function summarizeSections(sections: Section[]): DiffSummary {
  const summary = {
    sectionCount: sections.length,
    equalSections: 0,
    insertSections: 0,
    deleteSections: 0,
    changeSections: 0,
    equalLines: 0,
    insertedLines: 0,
    deletedLines: 0,
  };
  for (const s of sections) {
    if (s.kind === 'equal') {
      summary.equalSections++;
      summary.equalLines += (s.left?.end ?? 0) - (s.left?.start ?? 0);
    } else if (s.kind === 'insert') {
      summary.insertSections++;
      summary.insertedLines += (s.right?.end ?? 0) - (s.right?.start ?? 0);
    } else if (s.kind === 'delete') {
      summary.deleteSections++;
      summary.deletedLines += (s.left?.end ?? 0) - (s.left?.start ?? 0);
    } else {
      summary.changeSections++;
    }
  }
  return summary;
}
