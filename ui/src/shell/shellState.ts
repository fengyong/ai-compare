import type { SessionKind } from '../session/model';
import type { DirRow } from '../protocol/types';
import type { CloseGuardKind, PendingClose } from './closeGuard';

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

/** P14：表格比较标签数据 */
export interface TableTabData {
  left: string;
  right: string;
  sessionId: string;
}

/** P14：十六进制比较标签数据 */
export interface HexTabData {
  left: string;
  right: string;
  sessionId: string;
}

/** P7：三向文本合并标签数据（base=公共祖先/中间，left/right 两侧 + 输出） */
export interface Merge3TabData {
  base: string;
  left: string;
  right: string;
  /** 客户端分配的 text/merge3/* 会话句柄 */
  sessionId: string;
  /** "合并到"目标文件（FR-TMRG-006）；首次保存前为空，由用户显式指定 */
  outputPath?: string;
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
  /** 父目录标签（子会话保存后回传父目录行，FR-SES-004 / R3-17） */
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
  /** home 主页 / folder 目录比较 / text 文本比较 / sync 文件夹同步 / merge 文件夹合并（P8）/ merge3 三向文本合并（P7） */
  kind: 'home' | 'folder' | 'text' | 'sync' | 'merge' | 'merge3' | 'picture' | 'table' | 'hex';
  title: string;
  /** 固定标签（右键菜单"固定"，FR-WIN-001） */
  pinned?: boolean;
  /** 命名会话有未保存修改（关闭时提示保存，FR-SES-001） */
  dirty?: boolean;
  /** 逐侧未落盘的文件编辑（R3-05：与命名会话无关，关闭/退出前必须提示，FR-TXT-020） */
  contentDirty?: { left: boolean; right: boolean };
  sessionLink?: SessionLink;
  folder?: FolderTabData;
  text?: TextTabData;
  sync?: SyncTabData;
  picture?: PictureTabData;
  table?: TableTabData;
  hex?: HexTabData;
  merge?: MergeTabData;
  merge3?: Merge3TabData;
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
  openTableTab(left: string, right: string, opts?: OpenTabOpts): void;
  openHexTab(left: string, right: string, opts?: OpenTabOpts): void;
  /** 三向文本合并标签（队列 3，FR-TMRG-001：base/left/right 三路 + 输出） */
  openMerge3Tab(base: string, left: string, right: string, opts?: OpenTabOpts): void;
  /** 文件夹合并标签（P8，FR-DMRG-001：三路 base/left/right） */
  openFolderMergeTab(base: string, left: string, right: string, opts?: OpenTabOpts): void;
  /** 独立文本比较（主页文本大按钮入口；子会话路径见 openChild） */
  openTextTab(leftPath: string, rightPath: string, opts?: OpenTabOpts): Promise<void>;
  /** 双击目录行 → 按格式列表选型打开子会话（FR-DIR-070 / FR-SEL-001：文本/表格/十六进制/图片） */
  openChild(parentTabId: string, row: Pick<DirRow, 'id' | 'name' | 'leftPath' | 'rightPath'>): Promise<void>;
  activate(tabId: string): void;
  /** 另存文件为后改指标签上的该侧路径/名称/标题（R3-04：会话保存与关闭守卫据此取正确目标） */
  retargetTextSide(tabId: string, side: 'left' | 'right', path: string): void;
  /** 子会话差异文件遍历（FR-DIR-071）：跳到父会话下一/上一差异文件对，可先整文件复制到对侧 */
  navigateDiffFile(tabId: string, dir: 'next' | 'prev', copyTo?: 'left' | 'right'): Promise<void>;
  /** 记录三向合并的"合并到"目标（R3-20：下次保存默认预填该路径） */
  setMerge3OutputPath(tabId: string, path: string): void;
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
  /** 标记逐侧未落盘编辑（R3-05：文本视图编辑/保存后同步，关闭守卫据此判定） */
  markContentDirty(tabId: string, sides: { left: boolean; right: boolean }): void;
  /** 交换左右路径（演示脏标记/保存闭环的可视动作） */
  swapSides(tabId: string): void;
  closeGuard: { tabId: string; kind: CloseGuardKind } | null;
  /** 守卫处理完后要继续的批量动作（否则确认一次只关一个标签，批量动作静默中止） */
  pendingClose: PendingClose | null;
  exitHandler: (() => void) | null;
  resolveCloseGuard(choice: 'save' | 'discard' | 'cancel'): Promise<void>;
  /** 把标签上未落盘的文件编辑写回磁盘（关闭守卫"保存并关闭"用；false=未全部落盘） */
  saveContentForTab(tabId: string): Promise<boolean>;
  /** 原生窗口关闭/退出请求：逐个处理未保存项，全部处理完才真正退出 */
  requestAppExit(): Promise<void>;
  /** 真正销毁原生窗口的出口（Tauri 壳注入；浏览器演示模式为 null） */
  setExitHandler(fn: (() => void) | null): void;
}

/** 通用输入对话框（重命名会话/另存名等） */
export interface PromptDialogState {
  title: string;
  initialValue?: string;
  /** null = 取消 */
  onSubmit(value: string | null): void;
}

/** 通用确认对话框（R3-20 覆盖警示等需要"是/否"回答的场景） */
export interface ConfirmDialogState {
  title: string;
  message: string;
  confirmLabel?: string;
  onResolve(ok: boolean): void;
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
  confirm: ConfirmDialogState | null;
  /** 弹出确认框并等待回答；已有未决确认时先按取消结清，避免调用方永久挂起 */
  askConfirm(init: Omit<ConfirmDialogState, 'onResolve'>): Promise<boolean>;
  resolveConfirm(ok: boolean): void;
  /** 队列 4：剪贴板历史面板（FR-CLIP-001） */
  clipboardPanelOpen: boolean;
  toggleClipboardPanel(open?: boolean): void;
  /** 队列 5：报告对话框（FR-RPT-001） */
  reportDialog: { sessionId: string; kind: string } | null;
  openReportDialog(): void;
  closeReportDialog(): void;
  /** 队列 8：工具栏布局（FR-UI-003：大按钮 / 紧凑 / 隐藏） */
  toolbarLayout: 'large' | 'compact' | 'hidden';
  cycleToolbarLayout(): void;
}

export type ShellStore = WindowsSlice & TabsSlice & SessionSlice & UiSlice;
