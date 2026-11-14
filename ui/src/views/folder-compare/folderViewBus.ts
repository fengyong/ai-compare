/**
 * 目录比较视图的壳级命令入口（FR-DIR-050 空格标记 / 附录 G Ctrl+L、Ctrl+R）。
 * 与 textViewBus 同一口径：视图挂载时注册动作、卸载时按 token 注销，命令表/
 * 快捷键经此调用，避免把视图内部选择集提升到全局 store。
 *
 * 多窗口/多标签：注册为栈，后挂载的视图优先接命令；某个视图卸载只摘除自己的
 * 条目，不会把仍在挂载的其它目录视图的注册一并清空。
 */
export interface FolderViewActions {
  /** 复制选中项到左侧（Ctrl+L） */
  copyToLeft(): void;
  /** 复制选中项到右侧（Ctrl+R） */
  copyToRight(): void;
  /** 空格标记：切换选中行的标记集 */
  markSelection(): void;
}

interface Registration {
  token: number;
  actions: FolderViewActions;
}

let stack: Registration[] = [];
let seq = 0;

/** 注册并返回一个 disposer（useEffect 直接 return 即可） */
export function registerFolderView(actions: FolderViewActions): () => void {
  const token = ++seq;
  stack.push({ token, actions });
  return () => {
    stack = stack.filter((r) => r.token !== token);
  };
}

/** 当前接收命令的视图（栈顶 = 最近挂载） */
export function activeFolderView(): FolderViewActions | null {
  return stack.length > 0 ? stack[stack.length - 1].actions : null;
}

/** 仅供单测：彻底重置栈 */
export function resetFolderViewBusForTest(): void {
  stack = [];
  seq = 0;
}
