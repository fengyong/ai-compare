import { DEFAULT_BINDINGS, findConflicts, parseBinding, type BindingConflict } from './shortcuts';
import type { ViewCtx } from './viewRegistry';
import { useShell } from './store';
import { S } from '../strings';
import {
  captureClipboard,
  compareLatestClipboard,
  comparePathWithClipboard,
} from './clipboardActions';
import { activeTextView } from '../views/text-compare/textViewBus';
import { activeEditView } from '../views/text-edit/editViewBus';
import { activeFolderView } from '../views/folder-compare/folderViewBus';

/**
 * 命令注册表（docs/03 §3.4.2：菜单/工具栏/快捷键统一来源，对齐 SRS 附录 F 命令矩阵）。
 *
 * 会话/窗口/标签/设置命令全量可用；视图专属命令（复制到对侧、空格标记、换行、
 * 书签等）经视图总线（folderViewBus / textViewBus）调用视图内真实操作——选择集
 * 与光标是视图内状态，不提升到全局 store。
 */

import type { Tab } from './shellState';

export type CommandCategory = '会话' | '搜索' | '视图' | '窗口' | '工具';

export interface CommandDef {
  id: string;
  title: string;
  category: CommandCategory;
  /** 默认快捷键（DEFAULT_BINDINGS 键位表；附录 G 明示 + 壳级惯例） */
  defaultKeys?: string;
  /** 交付阶段（占位命令注明后续阶段） */
  phaseNote?: string;
  /** 视图可用性门控（如 folder.* 仅在目录视图可用） */
  when?(ctx: CommandContext): boolean;
  run(ctx: CommandContext): void | Promise<void>;
}

/** 命令运行上下文 = ViewCtx + 活动标签信息（壳在每次触发时构建） */
export interface CommandContext extends ViewCtx {
  /** 直接取 Tab['kind']：新增视图类型不再需要两处同步联合 */
  activeTabKind: Tab['kind'] | null;
  activeTabId: string | null;
}

export class CommandRegistry {
  private readonly commands = new Map<string, CommandDef>();

  register(def: CommandDef): void {
    if (this.commands.has(def.id)) throw new Error(`命令 id 重复: ${def.id}`);
    if (def.defaultKeys && !parseBinding(def.defaultKeys)) {
      throw new Error(`命令 ${def.id} 的默认键位非法: ${def.defaultKeys}`);
    }
    this.commands.set(def.id, def);
  }

  get(id: string): CommandDef | undefined {
    return this.commands.get(id);
  }

  list(): CommandDef[] {
    return [...this.commands.values()].sort(
      (a, b) => a.category.localeCompare(b.category) || a.title.localeCompare(b.title),
    );
  }

  /** 全部默认键位表（含未注册命令的占位键位） */
  defaultBindings(): Record<string, string> {
    return { ...DEFAULT_BINDINGS, ...this.collectDefaultKeys() };
  }

  private collectDefaultKeys(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const [id, def] of this.commands) {
      if (def.defaultKeys) out[id] = def.defaultKeys;
    }
    return out;
  }

  /** 当前默认键位的冲突清单（快捷键引擎构建期自检） */
  defaultKeyConflicts(): BindingConflict[] {
    return findConflicts(this.defaultBindings());
  }
}

/** 子会话门控：只有从文件夹会话打开的文件视图才有"差异文件"可遍历（FR-SES-004） */
function isChildFileTab(ctx: CommandContext): boolean {
  if (ctx.activeTabKind !== 'text' || !ctx.activeTabId) return false;
  return !!useShell.getState().findTab(ctx.activeTabId)?.tab.text?.parentFolderSessionId;
}

// ------------------------------------------------------------------
// 内置命令（P4 骨架：会话/窗口/标签/设置可用；视图专属注明阶段）
// ------------------------------------------------------------------

export function registerBuiltinCommands(reg: CommandRegistry): void {
  reg.register({
    id: 'session.save',
    title: '保存会话',
    category: '会话',
    defaultKeys: 'Ctrl+S',
    run: async (ctx) => {
      const st = useShell.getState();
      if (st.settingsDialog !== null) return;
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      const tab = win?.tabs.find((t) => t.id === win.activeTabId);
      if (!tab || tab.kind === 'home') {
        ctx.notice('当前标签不是会话，无法保存');
        return;
      }
      await st.saveSessionForTab(tab.id);
    },
  });

  reg.register({
    id: 'session.saveAs',
    title: '会话另存为',
    category: '会话',
    defaultKeys: 'Ctrl+Alt+S',
    run: async (ctx) => {
      const st = useShell.getState();
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      const tab = win?.tabs.find((t) => t.id === win.activeTabId);
      if (!tab || tab.kind === 'home') {
        ctx.notice('当前标签不是会话，无法另存');
        return;
      }
      st.saveAsForTab(tab.id);
    },
  });

  reg.register({
    id: 'session.lock',
    title: '锁定/解锁会话',
    category: '会话',
    defaultKeys: 'Ctrl+Alt+L',
    run: async () => {
      await useShell.getState().toggleLockCurrent();
    },
  });

  reg.register({
    id: 'session.swapSides',
    title: '交换左右路径',
    category: '会话',
    run: async () => {
      const st = useShell.getState();
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      const tab = win?.tabs.find((t) => t.id === win.activeTabId);
      if (tab && tab.kind !== 'home') st.swapSides(tab.id);
    },
  });

  reg.register({
    id: 'app.newTab',
    title: '新建标签页（主页）',
    category: '窗口',
    defaultKeys: 'Ctrl+T',
    run: async () => {
      const st = useShell.getState();
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      const home = win?.tabs.find((t) => t.kind === 'home');
      if (home) st.activate(home.id);
    },
  });

  reg.register({
    id: 'app.newWindow',
    title: '新建窗口',
    category: '窗口',
    defaultKeys: 'Ctrl+N',
    run: async () => {
      useShell.getState().openNewWindow();
    },
  });

  reg.register({
    id: 'app.closeTab',
    title: '关闭标签页',
    category: '窗口',
    defaultKeys: 'Ctrl+W',
    run: async () => {
      const st = useShell.getState();
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      if (win) await st.closeTab(win.activeTabId);
    },
  });

  reg.register({
    id: 'tab.next',
    title: '下一个标签页',
    category: '窗口',
    defaultKeys: 'Ctrl+PageDown',
    run: async () => cycleTab(1),
  });

  reg.register({
    id: 'tab.prev',
    title: '上一个标签页',
    category: '窗口',
    defaultKeys: 'Ctrl+PageUp',
    run: async () => cycleTab(-1),
  });

  reg.register({
    id: 'window.mergeAll',
    title: '合并所有窗口',
    category: '窗口',
    defaultKeys: 'Ctrl+Alt+M',
    run: async () => {
      useShell.getState().mergeAllWindows();
    },
  });

  reg.register({
    id: 'window.detachTab',
    title: '移动标签页到新窗口',
    category: '窗口',
    defaultKeys: 'Ctrl+Alt+N',
    run: async () => {
      const st = useShell.getState();
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      if (win) st.detachTab(win.activeTabId);
    },
  });

  reg.register({
    id: 'app.settings',
    title: '设置',
    category: '工具',
    defaultKeys: 'Ctrl+K',
    run: async () => {
      useShell.getState().openSettings({});
    },
  });

  reg.register({
    id: 'app.showSessionPanel',
    title: '显示/隐藏会话管理面板',
    category: '视图',
    defaultKeys: 'F9',
    run: async (ctx) => {
      const next = !(ctx.sessions.getUiState().showSessionPanel !== false);
      await ctx.sessions.setUiState({ showSessionPanel: next });
    },
  });

  // ---- 目录视图命令（附录 G 明示键位；经 folderViewBus 调用视图内真实操作） ----

  reg.register({
    id: 'folder.copyToLeft',
    title: '复制到左侧',
    category: '视图',
    defaultKeys: 'Ctrl+L',
    when: (ctx) => ctx.activeTabKind === 'folder',
    run: () => activeFolderView()?.copyToLeft(),
  });

  reg.register({
    id: 'folder.copyToRight',
    title: '复制到右侧',
    category: '视图',
    defaultKeys: 'Ctrl+R',
    when: (ctx) => ctx.activeTabKind === 'folder',
    run: () => activeFolderView()?.copyToRight(),
  });

  reg.register({
    id: 'folder.markSelection',
    title: '标记选择（比较不同名文件）',
    category: '视图',
    defaultKeys: 'Space',
    when: (ctx) => ctx.activeTabKind === 'folder',
    run: () => activeFolderView()?.markSelection(),
  });

  // ---- 搜索菜单：子会话差异文件遍历（FR-DIR-071，仅文件夹子会话可用） ----

  const childDiffNav = (ctx: CommandContext, dir: 'next' | 'prev', copyTo?: 'left' | 'right') => {
    if (ctx.activeTabId) void useShell.getState().navigateDiffFile(ctx.activeTabId, dir, copyTo);
  };

  reg.register({
    id: 'search.nextDiffFile',
    title: '下一个差异文件',
    category: '搜索',
    defaultKeys: 'Ctrl+Alt+Down',
    when: isChildFileTab,
    run: (ctx) => childDiffNav(ctx, 'next'),
  });

  reg.register({
    id: 'search.prevDiffFile',
    title: '上一个差异文件',
    category: '搜索',
    defaultKeys: 'Ctrl+Alt+Up',
    when: isChildFileTab,
    run: (ctx) => childDiffNav(ctx, 'prev'),
  });

  reg.register({
    id: 'search.copyToRightNext',
    title: '复制文件到右侧并打开下一个差异',
    category: '搜索',
    when: isChildFileTab,
    run: (ctx) => childDiffNav(ctx, 'next', 'right'),
  });

  reg.register({
    id: 'search.copyToLeftNext',
    title: '复制文件到左侧并打开下一个差异',
    category: '搜索',
    when: isChildFileTab,
    run: (ctx) => childDiffNav(ctx, 'next', 'left'),
  });

  reg.register({
    id: 'app.clipboardCompare',
    title: '剪贴板比较（最近两条内容）',
    category: '工具',
    defaultKeys: 'Ctrl+Alt+C',
    phaseNote: 'FR-CLIP-002：最近两条剪贴板历史在新文本比较中打开',
    run: async (ctx) => {
      await captureClipboard(ctx.clipboard);
      await compareLatestClipboard(ctx.notice);
    },
  });

  reg.register({
    id: 'app.clipboardHistory',
    title: '剪贴板历史',
    category: '工具',
    phaseNote: 'FR-CLIP-001：最近条目列表 + 另存/删除/比较/文本编辑打开',
    run: () => useShell.getState().toggleClipboardPanel(),
  });

  reg.register({
    id: 'app.report',
    title: '会话报告',
    category: '工具',
    phaseNote: 'FR-RPT-001/002：报告对话框（布局/布局级选项/输出目标）',
    when: (ctx) => ctx.activeTabKind !== null && ctx.activeTabKind !== 'home',
    run: () => useShell.getState().openReportDialog(),
  });

  reg.register({
    id: 'app.toolbarLayout',
    title: '切换工具栏布局',
    category: '工具',
    phaseNote: 'FR-UI-003：大按钮 / 紧凑 / 隐藏循环切换（右键工具栏同效）',
    run: () => useShell.getState().cycleToolbarLayout(),
  });

  // 队列 6：文本视图命令（FR-TXT-002 自动换行 / FR-TXT-023 书签 0-9）
  const textWhen = (ctx: CommandContext) => ctx.activeTabKind === 'text';
  reg.register({
    id: 'text.toggleWrap',
    title: '自动换行',
    category: '视图',
    when: textWhen,
    run: () => activeTextView()?.toggleWrap(),
  });
  reg.register({
    id: 'text.toggleBookmark',
    title: '切换书签（0-9）',
    category: '视图',
    defaultKeys: 'Ctrl+F2',
    when: textWhen,
    run: () => activeTextView()?.toggleBookmark(),
  });
  reg.register({
    id: 'text.nextBookmark',
    title: '转到下一个书签',
    category: '视图',
    defaultKeys: 'F2',
    when: textWhen,
    run: () => activeTextView()?.gotoBookmark(1),
  });
  reg.register({
    id: 'text.prevBookmark',
    title: '转到上一个书签',
    category: '视图',
    defaultKeys: 'Shift+F2',
    when: textWhen,
    run: () => activeTextView()?.gotoBookmark(-1),
  });
  reg.register({
    id: 'text.clearBookmarks',
    title: '清除书签',
    category: '视图',
    when: textWhen,
    run: () => activeTextView()?.clearBookmarks(),
  });

  // R3-04：文件落盘（FR-TXT-020）。BC 帮助未明示"保存文件"的默认键位，
  // 此处取 Ctrl+Shift+S（Ctrl+S 已被"保存会话"占用，不改既有键位语义）。
  reg.register({
    id: 'text.saveFile',
    title: '保存文件',
    category: '会话',
    defaultKeys: 'Ctrl+Shift+S',
    when: textWhen,
    run: () => activeTextView()?.saveFile(),
  });
  reg.register({
    id: 'text.saveFileAs',
    title: '另存文件为',
    category: '会话',
    when: textWhen,
    run: () => activeTextView()?.saveFileAs(),
  });

  // D2：独立文本编辑（单文件）。
  // 键位说明：Ctrl+S 已被 session.save 占用（静态冲突检测不看 when 门控），
  // 因此编辑标签的 Ctrl+S 由 saveSessionForTab 分流到「保存文件」，
  // 这里只登记无默认键的命令入口 + 未被占用的 Ctrl+F。
  const editWhen = (ctx: CommandContext) => ctx.activeTabKind === 'edit';
  reg.register({
    id: 'edit.find',
    title: '查找替换（文本编辑）',
    category: '视图',
    defaultKeys: 'Ctrl+F',
    when: editWhen,
    run: () => activeEditView()?.openFind(),
  });
  reg.register({
    id: 'edit.toggleWrap',
    title: '自动换行（文本编辑）',
    category: '视图',
    when: editWhen,
    run: () => activeEditView()?.toggleWrap(),
  });
  reg.register({
    id: 'edit.saveFile',
    title: '保存文件（文本编辑）',
    category: '会话',
    phaseNote: 'FR-EDIT-001：Ctrl+S 在编辑标签上即保存文件（由会话保存分流）',
    when: editWhen,
    run: () => activeEditView()?.saveFile(),
  });
  reg.register({
    id: 'edit.saveFileAs',
    title: '另存文件为（文本编辑）',
    category: '会话',
    when: editWhen,
    run: () => activeEditView()?.saveFileAs(),
  });

  reg.register({
    id: 'text.compareWithClipboard',
    title: '与剪贴板比较',
    category: '视图',
    phaseNote: 'FR-CLIP-004：当前文本会话左侧文件与剪贴板文本比较',
    when: (ctx) => ctx.activeTabKind === 'text',
    run: async (ctx) => {
      const st = useShell.getState();
      const win = st.windows.find((w) => w.id === st.activeWindowId);
      const tab = win?.tabs.find((t) => t.id === win.activeTabId);
      const left = tab?.text?.left.path;
      if (!left) {
        ctx.notice(S.clip.noLeftFile);
        return;
      }
      await comparePathWithClipboard(left, ctx.clipboard, ctx.notice);
    },
  });
}

async function cycleTab(delta: 1 | -1): Promise<void> {
  const st = useShell.getState();
  const win = st.windows.find((w) => w.id === st.activeWindowId);
  if (!win || win.tabs.length === 0) return;
  const idx = win.tabs.findIndex((t) => t.id === win.activeTabId);
  const next = (idx + delta + win.tabs.length) % win.tabs.length;
  st.activate(win.tabs[next].id);
}

/** 应用级单例 */
export const commandRegistry: CommandRegistry = (() => {
  const reg = new CommandRegistry();
  registerBuiltinCommands(reg);
  return reg;
})();
