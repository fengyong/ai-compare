import type { SessionKind } from '../session/model';

/**
 * 应用壳状态类型（M30）：多窗口 × 多标签（FR-WIN-001/002）。
 *
 * 结构：windows[]（每窗口独立 tabs/activeTabId）+ activeWindowId。
 * 浏览器演示模式下"窗口"以应用内窗口帧呈现（窗口切换条 + 拖出/合并行为完整）；
 * 原生多窗口（Tauri WebviewWindow + 跨窗口状态同步）属 P13 收敛项。
 *
 * zustand slice 化：windows / tabs / sessions / ui 四个 slice
 * （见 sliceWindows.ts / sliceTabs.ts / sliceSessions.ts / sliceUi.ts）。
 */

/** 文本子会话的文件引用（UI 侧视图模型；协议侧为纯路径字符串） */
export interface FileRef {
  path: string;
  name: string;
}

export interface FolderTabData {
  left: string;
  right: string;
  /** 客户端分配的引擎会话句柄（生成协议 DirOpenParams.sessionId），标签创建时即确定 */
  sessionId: string;
}

/** 文件夹同步标签数据（P8：复用目录会话 + 同步方法/策略） */
export interface SyncTabData {
  left: string;
  right: string;
  sessionId: string;
}
/** P14：图片比较标签数据（两侧路径 + 引擎会话句柄） */
export interface PictureTabData {
  left: string;
  right: string;
  sessionId: string;
}

/** 文件夹合并标签数据（P8：三路 base/left/right） */
export interface MergeTabData {
  base: string;
  left: string;
  right: string;
  /** 客户端分配的合并会话句柄（dir/mergeScan 提案协议） */
  sessionId: string;
}

export interface TextTabData {
  sessionId: string;
  left: FileRef;
  right: FileRef;
  /** 父目录标签（差异回传链路 docs/03 §5.1 的 UI 侧关联；引擎侧写回属 P1） */
  parentTabId?: string;
  parentFolderSessionId?: string;
}

/** 标签与命名会话的关联（保存/另存/打开已保存会话后存在，SES-001） */
export interface SessionLink {
  /** sessions.json 中的会话 id */
  sessionId: string;
  name: string;
  kind: SessionKind;
  /** 锁定会话：状态修改不保存、退出不提示（提示在打开时给出一次） */
  locked: boolean;
}

export interface Tab {
  id: string;
  /** home 主页 / folder 目录比较 / text 文本比较 / sync 文件夹同步 / merge 文件夹合并（P8） */
  kind: 'home' | 'folder' | 'text' | 'sync' | 'merge' | 'picture';
  title: string;
  /** 固定标签（右键菜单"固定"，FR-WIN-001） */
  pinned?: boolean;
  /** 命名会话有未保存修改（关闭时提示保存，FR-SES-001） */
  dirty?: boolean;
  sessionLink?: SessionLink;
  folder?: FolderTabData;
  text?: TextTabData;
  sync?: SyncTabData;
  picture?: PictureTabData;
  merge?: MergeTabData;
}

export interface WindowState {
  id: string;
  /** 窗口标题 = 活动标签标题（激活时联动） */
  title: string;
  tabs: Tab[];
  activeTabId: string;
}

/** 设置对话框打开状态（shell 顶层对话框编排） */
export interface SettingsDialogState {
  /** 初始页 id */
  page?: string;
  /** 会话作用域模式：指定类型 + 会话 id（null 表示"编辑类型默认"入口） */
  kind?: SessionKind;
  sessionTabId?: string;
}

// ------------------------------------------------------------------
// ShellStore 组合类型（各 slice 的字段与动作全集）
// ------------------------------------------------------------------

export interface WindowsSlice {
  windows: WindowState[];
  activeWindowId: string;
  /** 窗口 id 序列（w-1 为主窗口） */
  openNewWindow(opts?: { withHomeTab?: boolean }): void;
  activateWindow(windowId: string): void;
  /** 标签拖出成新窗口（FR-WIN-001 移动标签页到新窗口；home 标签不可拖出） */
  detachTab(tabId: string): void;
  /** 合并所有窗口到主窗口（FR-WIN-001 合并所有窗口；其余窗口的 home 标签丢弃） */
  mergeAllWindows(): void;
  /** 关闭窗口（先逐标签走关闭流程；最后一个窗口关闭后回到全新主窗口） */
  closeWindow(windowId: string): Promise<void>;
  windowCount(): number;
}

export interface TabsSlice {
  openFolderTab(left: string, right: string, opts?: OpenTabOpts): void;
  /** 文件夹同步标签（P8，FR-SYNC-001：复用目录会话句柄） */
  openFolderSyncTab(left: string, right: string, opts?: OpenTabOpts): void;
  openPictureTab(left: string, right: string, opts?: OpenTabOpts): void;
  /** 文件夹合并标签（P8，FR-DMRG-001：三路 base/left/right） */
  openFolderMergeTab(base: string, left: string, right: string, opts?: OpenTabOpts): void;
  /** 独立文本比较（主页文本大按钮入口；子会话路径见 openTextChild） */
  openTextTab(leftPath: string, rightPath: string, opts?: OpenTabOpts): Promise<void>;
  /** 双击目录行 → 打开文本子会话（P0 闭环，保持原语义） */
  openTextChild(parentTabId: string, row: { id: string; name: string }): Promise<void>;
  activate(tabId: string): void;
  /** 关闭（命名会话有未保存修改时转 closeGuard；引擎会话幂等清理） */
  closeTab(tabId: string): Promise<void>;
  /** 关闭其他标签（保留指定标签与各窗口 home） */
  closeOthers(tabId: string): Promise<void>;
  togglePin(tabId: string): void;
  /** 标签在窗口内拖拽重排 */
  reorderTab(tabId: string, toIndex: number): void;
  /** 引擎重连后重绑会话句柄（resync 降级路径：全量重放） */
  rebindSession(oldSessionId: string, newSessionId: string): void;
  findTab(tabId: string): { tab: Tab; windowId: string } | undefined;
}

export interface OpenTabOpts {
  windowId?: string;
  sessionLink?: SessionLink;
  activate?: boolean;
}

export interface SessionSlice {
  /** 从会话树打开已保存会话（SES-005 加载会话；锁定时提示，FR-SES-001） */
  openSavedSession(sessionId: string): Promise<void>;
  /** 命名会话保存（当前标签 → sessionLink 更新；未关联时转另存流程） */
  saveSessionForTab(tabId: string): Promise<void>;
  /** 另存为对话框 */
  saveAsPrompt: { tabId: string } | null;
  saveAsForTab(tabId: string): void;
  resolveSaveAs(name: string | null): Promise<void>;
  /** 会话树右键重命名（输入对话框） */
  renameSessionPrompt(sessionId: string): void;
  /** 会话锁定切换（SES-001） */
  toggleLockCurrent(): Promise<void>;
  /** 标记脏（命名会话主要参数被修改） */
  markDirty(tabId: string, dirty: boolean): void;
  /** 交换左右路径（演示脏标记/保存闭环的可视动作） */
  swapSides(tabId: string): void;
  closeGuard: { tabId: string } | null;
  resolveCloseGuard(choice: 'save' | 'discard' | 'cancel'): Promise<void>;
}

/** 通用输入对话框（重命名会话/另存名等） */
export interface PromptDialogState {
  title: string;
  initialValue?: string;
  /** null = 取消 */
  onSubmit(value: string | null): void;
}

export interface UiSlice {
  notice: string | null;
  setNotice(msg: string | null): void;
  settingsDialog: SettingsDialogState | null;
  openSettings(init?: SettingsDialogState): void;
  closeSettings(): void;
  prompt: PromptDialogState | null;
  openPrompt(init: PromptDialogState): void;
  resolvePrompt(value: string | null): void;
}

export type ShellStore = WindowsSlice & TabsSlice & SessionSlice & UiSlice;
