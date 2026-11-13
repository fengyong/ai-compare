/**
 * 会话级撤销/重做栈（UNDO-001）。
 *
 * 独立于 CM6 history（两侧窗格共享一个栈，撤销跨窗格生效——BC 语义）：
 * - 条目 = 单侧内容补丁集（editPatch.ts 的复合载荷）；
 * - 应用 = 直接改单侧内容行 → 触发整会话重 diff（视图层编排）；
 * - LIFO 撤销/重做天然成立（每条目的补丁坐标即"入栈时状态"的坐标）；
 * - 连续输入合并（同侧、纯追加、时间窗内）为一个条目（打字不逐键膨胀）。
 */

import type { Side } from '../../protocol/types';
import { applySidePatches, invertSidePatches, type SidePatch } from './editPatch';

export interface UndoEntry {
  side: Side;
  patches: SidePatch[];
  /** 合并判定用（仅 UI 层写入） */
  lastAt?: number;
}

export interface UndoState {
  undo: UndoEntry[];
  redo: UndoEntry[];
}

export const UNDO_LIMIT = 200;

export function emptyUndoState(): UndoState {
  return { undo: [], redo: [] };
}

/** 压入一条编辑（清除重做链；超出上限丢最旧） */
export function pushUndo(st: UndoState, entry: UndoEntry): UndoState {
  const undo = [...st.undo, entry];
  if (undo.length > UNDO_LIMIT) undo.splice(0, undo.length - UNDO_LIMIT);
  return { undo, redo: [] };
}

/**
 * 尝试合并连续输入（时间窗内，BC 打字节奏）：
 * 1. 纯追加链：同侧单补丁纯插入，且新插入紧跟上次插入末尾（空行打字）；
 * 2. 同点改行链：同侧单补丁单行替换，start 相同且 removed 链相接
 *   （在已有行内连续输入，中间态折叠为 原→终）。
 */
export function tryCoalesce(st: UndoState, entry: UndoEntry, now: number, maxGapMs = 1000): UndoState {
  const top = st.undo[st.undo.length - 1];
  if (
    top &&
    entry.lastAt !== undefined &&
    top.lastAt !== undefined &&
    now - top.lastAt <= maxGapMs &&
    top.side === entry.side &&
    top.patches.length === 1 &&
    entry.patches.length === 1
  ) {
    const prev = top.patches[0];
    const next = entry.patches[0];
    // 1) 纯追加链
    if (
      prev.removed.length === 0 &&
      next.removed.length === 0 &&
      next.start === prev.start + prev.inserted.length
    ) {
      const merged: UndoEntry = {
        side: top.side,
        patches: [{ start: prev.start, removed: [], inserted: [...prev.inserted, ...next.inserted] }],
        lastAt: entry.lastAt,
      };
      return { undo: [...st.undo.slice(0, -1), merged], redo: [] };
    }
    // 2) 同点改行链：单行 → 单行，且本次被替换行 == 上次替换结果首行
    if (
      prev.removed.length === 1 &&
      prev.inserted.length >= 1 &&
      next.removed.length === 1 &&
      next.inserted.length === 1 &&
      next.start === prev.start &&
      next.removed[0] === prev.inserted[prev.inserted.length - 1]
    ) {
      const merged: UndoEntry = {
        side: top.side,
        patches: [{ start: prev.start, removed: prev.removed, inserted: next.inserted }],
        lastAt: entry.lastAt,
      };
      return { undo: [...st.undo.slice(0, -1), merged], redo: [] };
    }
  }
  return pushUndo(st, entry);
}

export function canUndo(st: UndoState): boolean {
  return st.undo.length > 0;
}

export function canRedo(st: UndoState): boolean {
  return st.redo.length > 0;
}

/** 弹出撤销条目（调用方对当前行内容 applyPatches(invert)） */
export function popUndo(st: UndoState): { state: UndoState; entry: UndoEntry } | null {
  if (st.undo.length === 0) return null;
  const entry = st.undo[st.undo.length - 1];
  return { state: { undo: st.undo.slice(0, -1), redo: [...st.redo, entry] }, entry };
}

/** 弹出重做条目（调用方对当前行内容 applyPatches(entry.patches)） */
export function popRedo(st: UndoState): { state: UndoState; entry: UndoEntry } | null {
  if (st.redo.length === 0) return null;
  const entry = st.redo[st.redo.length - 1];
  return { state: { undo: [...st.undo, entry], redo: st.redo.slice(0, -1) }, entry };
}

/** 撤销应用的补丁（已反演 + 倒序） */
export function undoPatches(entry: UndoEntry): SidePatch[] {
  return invertSidePatches(entry.patches);
}

/** 重做应用的补丁（原样） */
export function redoPatches(entry: UndoEntry): SidePatch[] {
  return entry.patches;
}

export { applySidePatches };
