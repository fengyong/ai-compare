/**
 * 文本编辑视图的壳级命令入口（D2，FR-EDIT-001）。
 *
 * 与 `views/text-compare/textViewBus` 同一套约定：视图挂载时注册动作、卸载时
 * 按 token 注销；命令表/快捷键经此调用，避免把编辑器内部状态（查找面板、
 * 换行开关）提升到全局 store。注册为栈，后挂载的视图优先接命令。
 */
export interface EditViewActions {
  /** 打开/聚焦查找替换条 */
  openFind(): void;
  /** 切换自动换行 */
  toggleWrap(): void;
  /** 保存文件（与壳级 saveContentForTab 同一份落盘语义） */
  saveFile(): void;
  /** 另存文件为 */
  saveFileAs(): void;
}

interface Registration {
  token: number;
  actions: EditViewActions;
}

let stack: Registration[] = [];
let seq = 0;

/** 注册并返回一个 disposer（useEffect 直接 return 即可） */
export function registerEditView(actions: EditViewActions): () => void {
  const token = ++seq;
  stack.push({ token, actions });
  return () => {
    stack = stack.filter((r) => r.token !== token);
  };
}

/** 当前接收命令的视图（栈顶 = 最近挂载） */
export function activeEditView(): EditViewActions | null {
  return stack.length > 0 ? stack[stack.length - 1].actions : null;
}

/** 仅供单测：彻底重置栈 */
export function resetEditViewBusForTest(): void {
  stack = [];
  seq = 0;
}
