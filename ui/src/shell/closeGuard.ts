import type { Tab, WindowState } from './shellState';

/**
 * 关闭守卫判定（R3-05，FR-SES-001 + FR-TXT-020）。
 *
 * 两类未保存修改的丢弃代价不同：文件编辑丢了就是丢数据，命名会话参数丢了只是
 * 丢视图状态；因此 content 优先于 session，且 content 不以"是否命名会话"为前提
 * （未命名标签的编辑同样必须提示）。
 */
export type CloseGuardKind = 'content' | 'session';

export interface PendingGuard {
  tabId: string;
  kind: CloseGuardKind;
}

/** 守卫处理完后要继续的批量动作（否则确认一次只关掉一个标签，批量动作静默中止） */
export type PendingClose =
  { kind: 'window'; windowId: string } | { kind: 'others'; tabId: string } | { kind: 'exit' };

export function hasContentEdits(tab: Tab): boolean {
  return tab.contentDirty !== undefined && (tab.contentDirty.left || tab.contentDirty.right);
}

export function closeGuardKind(tab: Tab): CloseGuardKind | null {
  if (tab.kind === 'home') return null;
  if (hasContentEdits(tab)) return 'content';
  if (tab.sessionLink && tab.dirty) return 'session';
  return null;
}

export function guardOf(tab: Tab): PendingGuard | null {
  const kind = closeGuardKind(tab);
  return kind === null ? null : { tabId: tab.id, kind };
}

/** 按窗口/标签顺序取第一个需要守卫的标签（批量关闭与退出逐个提示） */
export function firstPendingGuard(windows: WindowState[]): PendingGuard | null {
  for (const w of windows) {
    for (const t of w.tabs) {
      const g = guardOf(t);
      if (g) return g;
    }
  }
  return null;
}

/** 保存后剩余的未落盘侧（只清除真正落盘的侧） */
export function remainingSides(
  dirty: { left: boolean; right: boolean },
  saved: ReadonlyArray<{ side: 'left' | 'right' }>,
): { left: boolean; right: boolean } {
  const out = { left: dirty.left, right: dirty.right };
  for (const e of saved) out[e.side] = false;
  return out;
}
