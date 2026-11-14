/**
 * 文本比较视图的壳级命令入口（FR-TXT-002 自动换行 / FR-TXT-023 书签）。
 * 视图挂载时注册动作，卸载时注销；命令表/快捷键经此调用，避免把视图内部
 * state 提升到全局 store（书签与换行均为会话视图内状态）。
 */
export interface TextViewActions {
  toggleWrap(): void;
  toggleBookmark(): void;
  gotoBookmark(direction: 1 | -1): void;
  clearBookmarks(): void;
  setBookmarkAtCursor(slot: number): void;
}

let active: TextViewActions | null = null;

export function setActiveTextView(actions: TextViewActions | null): void {
  active = actions;
}

export function activeTextView(): TextViewActions | null {
  return active;
}
