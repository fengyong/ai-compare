/**
 * 文本比较书签（docs/01 FR-TXT-023）：编号标记 0-9、切换/转到/清除。
 * 纯函数，行号为 0 基对齐行号（与 AlignedDoc.rows 坐标一致）。
 */

/** slot（0-9）→ 0 基行号 */
export type BookmarkMap = Record<number, number>;

export const BOOKMARK_SLOTS: readonly number[] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

export function bookmarkSlotAtLine(map: BookmarkMap, line: number): number | null {
  for (const slot of BOOKMARK_SLOTS) {
    if (map[slot] === line) return slot;
  }
  return null;
}

export interface ToggleResult {
  map: BookmarkMap;
  /** 被清除或新增的编号；十个编号全满且当前行无书签时为 null */
  slot: number | null;
  added: boolean;
}

/**
 * 切换书签：当前行已有 → 清除该编号；否则分配最小空闲编号（0-9）。
 * BC 语义：切换书签在当前行放编号标记，重复切换即清除。
 */
export function toggleBookmark(map: BookmarkMap, line: number): ToggleResult {
  const existing = bookmarkSlotAtLine(map, line);
  if (existing !== null) {
    const next = { ...map };
    delete next[existing];
    return { map: next, slot: existing, added: false };
  }
  const free = BOOKMARK_SLOTS.find((s) => map[s] === undefined);
  if (free === undefined) return { map: { ...map }, slot: null, added: false };
  return { map: { ...map, [free]: line }, slot: free, added: true };
}

/** 在指定编号放置/移动书签（UI 里点编号小按钮直接设定） */
export function setBookmark(map: BookmarkMap, slot: number, line: number): BookmarkMap {
  if (!BOOKMARK_SLOTS.includes(slot)) return { ...map };
  return { ...map, [slot]: line };
}

export function clearBookmark(map: BookmarkMap, slot: number): BookmarkMap {
  const next = { ...map };
  delete next[slot];
  return next;
}

export function clearBookmarks(): BookmarkMap {
  return {};
}

export function bookmarkTargets(map: BookmarkMap): Array<{ slot: number; line: number }> {
  return BOOKMARK_SLOTS.filter((s) => typeof map[s] === 'number')
    .map((slot) => ({ slot, line: map[slot] }))
    .sort((a, b) => a.line - b.line || a.slot - b.slot);
}

/** 转到下一个书签（严格大于当前行的最小书签；无则循环回第一个） */
export function nextBookmarkLine(map: BookmarkMap, currentLine: number): number | null {
  const targets = bookmarkTargets(map);
  if (targets.length === 0) return null;
  return (targets.find((t) => t.line > currentLine) ?? targets[0]).line;
}

/** 转到上一个书签（严格小于当前行的最大书签；无则循环回最后一个） */
export function prevBookmarkLine(map: BookmarkMap, currentLine: number): number | null {
  const targets = bookmarkTargets(map);
  if (targets.length === 0) return null;
  for (let i = targets.length - 1; i >= 0; i--) {
    if (targets[i].line < currentLine) return targets[i].line;
  }
  return targets[targets.length - 1].line;
}

/** 文档行号（1 起）显示 */
export function bookmarkLabel(map: BookmarkMap): string {
  const targets = bookmarkTargets(map);
  return targets.length === 0 ? '' : targets.map((t) => `${t.slot}:${t.line + 1}`).join(' ');
}
