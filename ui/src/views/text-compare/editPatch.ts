/**
 * 编辑 → 单侧内容补丁（P5 编辑实时重 diff 的映射核心）。
 *
 * 难点：CM6 文档是"对齐文档"（含对侧缺失行的填充行），而重 diff 需要
 * 单侧文件行。本模块把"对齐文档的前后文本"换算成单侧内容的补丁：
 * - 对前后对齐文档做行级 diff（lineDiffCore）；
 * - 沿 op 序列游走，用对齐行元数据（AlignedRow）区分"真实行"与"填充行"：
 *   真实行变化 → 原行替换；填充行输入文本 → 新增单侧行；填充行清空 → 无贡献；
 * - 独立编辑块（多 hunk）产出多个补丁（撤销条目的复合载荷）。
 *
 * 补丁 start 采用"该侧内容数组在补丁应用时"的坐标（多 hunk 顺序应用成立，
 * 撤销按逆序反演），见 undoStack.ts。
 */

import { diffLinesOps } from '../../lib/lineDiffCore';
import { linesToEditText } from '../../lib/lineTable';
import type { Side, TextEditOp } from '../../protocol/types';
import type { AlignedRow } from './alignment';

export interface SidePatch {
  /** 应用时该侧内容数组中的起始行（0 基） */
  start: number;
  /** 被替换的原行（撤销时回填） */
  removed: string[];
  /** 替换后的新行 */
  inserted: string[];
}

export interface SidePatchResult {
  patches: SidePatch[];
  /** 该侧编辑后的完整内容行 */
  newLines: string[];
}

/** 行在侧内容数组中的索引取值（left → leftLine，right → rightLine） */
function sideIndexOf(
  row: AlignedRow | undefined,
  side: Side,
  fallbackCursor: number,
  sideLen: number,
): number | undefined {
  if (row) {
    const idx = side === 'left' ? row.leftLine : row.rightLine;
    if (idx !== undefined) return idx;
    return undefined; // 填充行：显式无贡献
  }
  // 元数据缺失（不一致状态防御）：尽量按游标延续，避免内容丢失
  return fallbackCursor < sideLen ? fallbackCursor : undefined;
}

/**
 * 由对齐文档编辑推导单侧内容补丁。
 * @param rows 编辑前对齐行元数据（与 docBefore 一一对应）
 * @param docBefore 编辑前对齐文档行
 * @param docAfter 编辑后对齐文档行
 * @param side 编辑发生在哪一侧（该侧为"编辑侧"）
 * @param sideLines 编辑前该侧内容行
 * @returns null 表示内容无变化（跳过重 diff）
 */
export function patchSideFromDocEdit(
  rows: AlignedRow[],
  docBefore: string[],
  docAfter: string[],
  side: Side,
  sideLines: string[],
): SidePatchResult | null {
  const ops = diffLinesOps(docBefore, docAfter);
  let changed = false;
  const newLines: string[] = [];
  const patches: SidePatch[] = [];
  let sideCursor = 0; // 原 sideLines 已消费的行数
  let cumDelta = 0; // 已产出补丁的净行数变化（start 坐标换算）
  let cur: { origStart: number; removed: string[]; inserted: string[] } | null = null;

  const beginHunk = (): void => {
    if (!cur) cur = { origStart: sideCursor, removed: [], inserted: [] };
  };
  const flush = (): void => {
    if (cur && (cur.removed.length > 0 || cur.inserted.length > 0)) {
      patches.push({
        start: cur.origStart + cumDelta,
        removed: cur.removed,
        inserted: cur.inserted,
      });
      cumDelta += cur.inserted.length - cur.removed.length;
      changed = true;
    }
    cur = null;
  };

  for (const op of ops) {
    if (op.type === 'equal') {
      flush();
      for (let i = op.aStart; i < op.aEnd; i++) {
        const idx = sideIndexOf(rows[i], side, sideCursor, sideLines.length);
        if (idx !== undefined) {
          newLines.push(sideLines[idx]);
          sideCursor++;
        }
        // 填充行未变 → 不产出该侧行
      }
    } else {
      // delete / insert / change：先处理旧行（删除侧），再处理新行（插入侧）
      if (op.aEnd > op.aStart) {
        beginHunk();
        for (let i = op.aStart; i < op.aEnd; i++) {
          const idx = sideIndexOf(rows[i], side, sideCursor, sideLines.length);
          if (idx !== undefined) {
            cur!.removed.push(sideLines[idx]);
            sideCursor++;
          }
        }
      }
      if (op.bEnd > op.bStart) {
        beginHunk();
        for (let j = op.bStart; j < op.bEnd; j++) {
          cur!.inserted.push(docAfter[j]);
          newLines.push(docAfter[j]); // 编辑侧新行进入新内容
        }
      }
    }
  }
  flush();

  if (!changed) return null;
  return { patches, newLines };
}

/** 顺序应用补丁（多 hunk 前向应用；redo 路径） */
export function applySidePatches(lines: string[], patches: SidePatch[]): string[] {
  let out = lines;
  for (const p of patches) {
    out = [...out.slice(0, p.start), ...p.inserted, ...out.slice(p.start + p.removed.length)];
  }
  return out;
}

/** 反演补丁（撤销用）：removed/inserted 互换，且顺序倒排（逆序应用） */
export function invertSidePatches(patches: SidePatch[]): SidePatch[] {
  return [...patches]
    .reverse()
    .map((p) => ({ start: p.start, removed: p.inserted, inserted: p.removed }));
}

/**
 * 单侧补丁 → text/edit 载荷（R3-18 引擎权威重 diff）。
 *
 * `start` 已是"应用时坐标"（见模块文档），与引擎按序应用同口径。
 * `text` **必须**走 `lib/lineTable.linesToEditText`：插入内容末尾是空行时，
 * `join('\n')` 会让引擎少解析一行（`from_text` 不把结尾换行当新行），
 * 于是行数校验失败 → 重放 → 仍不符 → 判分叉，比较视图里表现为"插入的空行
 * 保存后消失"（A1）。行尾风格（CRLF 等）仍由引擎按该侧既有风格归一。
 */
export function opsFromPatches(side: Side, patches: readonly SidePatch[]): TextEditOp[] {
  return patches.map((p) => ({
    side,
    start: p.start,
    end: p.start + p.removed.length,
    text: linesToEditText(p.inserted),
  }));
}

/** 整侧重放 op（end 省略 = 到末尾）：引擎态与 UI 态分叉后的收敛手段 */
export function fullSideReplayOp(side: Side, lines: readonly string[]): TextEditOp {
  return { side, start: 0, text: linesToEditText(lines) };
}

/**
 * 由"逐行替换后的整文件"推导复合补丁（"全部替换"路径）：
 * 新旧行数相同（替换不增删行），连续变化行归并为一个补丁块。
 * 无变化返回 null。
 */
export function patchesFromLineReplacement(
  oldLines: string[],
  newLines: string[],
): SidePatch[] | null {
  if (oldLines.length !== newLines.length) {
    // 行数不一致（不应发生；防御性回退为整体替换）
    if (oldLines.length === 0 && newLines.length === 0) return null;
    return [{ start: 0, removed: oldLines, inserted: newLines }];
  }
  const patches: SidePatch[] = [];
  let cur: { start: number; removed: string[]; inserted: string[] } | null = null;
  const flush = (): void => {
    if (cur) {
      patches.push(cur);
      cur = null;
    }
  };
  for (let i = 0; i < oldLines.length; i++) {
    if (oldLines[i] !== newLines[i]) {
      if (!cur) cur = { start: i, removed: [], inserted: [] };
      cur.removed.push(oldLines[i]);
      cur.inserted.push(newLines[i]);
    } else {
      flush();
    }
  }
  flush();
  return patches.length > 0 ? patches : null;
}
