/**
 * 文本比较视图的壳级命令入口（FR-TXT-002 自动换行 / FR-TXT-023 书签）。
 * 视图挂载时注册动作、卸载时按 token 注销；命令表/快捷键经此调用，避免把
 * 视图内部 state 提升到全局 store（书签与换行均为会话视图内状态）。
 *
 * 多窗口/多标签：注册为栈，后挂载的视图优先接命令；某个视图卸载只摘除
 * 自己的条目，不会把仍在挂载的其它文本视图的注册一并清空。
 */
export interface TextViewActions {
  toggleWrap(): void;
  toggleBookmark(): void;
  gotoBookmark(direction: 1 | -1): void;
  clearBookmarks(): void;
  setBookmarkAtCursor(slot: number): void;
  /** 保存文件（R3-04，FR-TXT-020）：写回所有有未落盘编辑的侧 */
  saveFile(): void;
  /** 另存文件为：作用于当前活动窗格侧 */
  saveFileAs(): void;
}

interface Registration {
  token: number;
  actions: TextViewActions;
}

let stack: Registration[] = [];
let seq = 0;

/** 注册并返回一个 disposer（useEffect 直接 return 即可） */
export function registerTextView(actions: TextViewActions): () => void {
  const token = ++seq;
  stack.push({ token, actions });
  return () => {
    stack = stack.filter((r) => r.token !== token);
  };
}

/** 当前接收命令的视图（栈顶 = 最近挂载） */
export function activeTextView(): TextViewActions | null {
  return stack.length > 0 ? stack[stack.length - 1].actions : null;
}

/** 兼容旧调用：null 清空全部注册；非 null 等价于注册但不返回 disposer */
export function setActiveTextView(actions: TextViewActions | null): void {
  if (actions === null) stack = [];
  else void registerTextView(actions);
}

/** 仅供单测：彻底重置栈 */
export function resetTextViewBusForTest(): void {
  stack = [];
  seq = 0;
}
