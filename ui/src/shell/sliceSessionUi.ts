import type { StateCreator } from 'zustand';
import { sessionService } from '../session/service';
import { S } from '../strings';
import type { SessionSlice, ShellStore } from './shellState';
import { sessionIdOfTab } from '../views/report/reportModel';
import { nextToolbarLayout } from './menuModel';
import { closeGuardKind, firstPendingGuard, hasContentEdits, remainingSides } from './closeGuard';
import { applySavedEntries, runTextSave, saveRequestsFor } from '../views/text-compare/textSave';
import { getTextSession } from '../views/text-compare/textModel';
import { getEditDoc, isDocDirty } from '../views/text-edit/editModel';
import { saveEditDoc, savedNotice } from '../views/text-edit/textEditSave';
import { sessionKindOfTab, specsOfTab } from './sessionSpecs';
import * as engine from '../ipc/engineProcess';

/** 会话 slice（SES-001~006）：打开已保存会话 / 保存 / 另存 / 锁定 / 关闭确认 */
export const createSessionSlice: StateCreator<ShellStore, [], [], SessionSlice> = (set, get) => ({
  closeGuard: null,
  pendingClose: null,
  exitHandler: null,
  saveAsPrompt: null,

  /** 从会话树打开已保存会话（SES-005「加载会话」从主页树打开） */
  async openSavedSession(sessionId) {
    const node = sessionService.getSession(sessionId);
    if (!node) {
      get().setNotice(S.session.notFound);
      return;
    }
    // 锁定会话重开提示（FR-SES-001：锁定后离开会话不保存显示过滤器等状态）
    if (node.locked) {
      get().setNotice(
        `${S.session.lockedNoticePrefix}「${node.name}」${S.session.lockedNoticeSuffix}`,
      );
    }
    const link = { sessionId: node.id, name: node.name, kind: node.kind, locked: node.locked };
    const { left, right } = node.specs;
    // 三路会话的 base：旧会话文件只存了 left/right 两位，缺 base 时以 left 兼作
    const base = node.specs.base ?? left;
    switch (node.kind) {
      case 'folder-compare':
        get().openFolderTab(left, right, { sessionLink: link });
        break;
      case 'text-compare':
        await get().openTextTab(left, right, { sessionLink: link });
        break;
      case 'text-edit':
        // D2：单文件会话只有 left 位
        await get().openEditTab(left, { sessionLink: link });
        break;
      case 'folder-sync':
        get().openFolderSyncTab(left, right, { sessionLink: link });
        break;
      case 'picture-compare':
        get().openPictureTab(left, right, { sessionLink: link });
        break;
      case 'table-compare':
        get().openTableTab(left, right, { sessionLink: link });
        break;
      case 'hex-compare':
        get().openHexTab(left, right, { sessionLink: link });
        break;
      case 'folder-merge':
        get().openFolderMergeTab(base, left, right, { sessionLink: link });
        break;
      case 'text-merge':
        get().openMerge3Tab(base, left, right, { sessionLink: link });
        break;
    }
    await sessionService.recordOpened(node.kind, node.specs, node.id);
  },

  /** 命名会话保存（SES-001：显式保存；未命名标签转另存流程） */
  async saveSessionForTab(tabId) {
    const found = get().findTab(tabId);
    if (!found) return;
    const tab = found.tab;
    // D2：编辑标签有未落盘内容时，"保存"（Ctrl+S）的直觉语义是保存文件
    if (tab.kind === 'edit' && hasContentEdits(tab)) {
      await get().saveContentForTab(tabId);
      return;
    }
    if (tab.sessionLink) {
      if (tab.sessionLink.locked) {
        get().setNotice(`${S.session.lockedSaveDenied}「${tab.sessionLink.name}」`);
        return;
      }
      await sessionService.saveSession(
        {
          kind: tab.sessionLink.kind,
          name: tab.sessionLink.name,
          specs: specsOfTab(tab),
          overrides: sessionService.getSession(tab.sessionLink.sessionId)?.overrides,
        },
        tab.sessionLink.sessionId,
      );
      set((st) => ({
        windows: st.windows.map((w) => ({
          ...w,
          tabs: w.tabs.map((t) => (t.id === tabId ? { ...t, dirty: false } : t)),
        })),
      }));
      get().setNotice(`${S.session.savedPrefix}「${tab.sessionLink.name}」`);
      return;
    }
    get().saveAsForTab(tabId);
  },

  saveAsForTab(tabId) {
    set({ saveAsPrompt: { tabId } });
  },

  async resolveSaveAs(name) {
    const prompt = get().saveAsPrompt;
    if (!prompt || name === null) {
      set({ saveAsPrompt: null });
      return;
    }
    const trimmed = name.trim();
    if (!trimmed) return; // 保持对话框打开
    const found = get().findTab(prompt.tabId);
    if (!found) {
      set({ saveAsPrompt: null });
      return;
    }
    const tab = found.tab;
    const kind = sessionKindOfTab(tab.kind);
    const node = await sessionService.saveSession({ kind, name: trimmed, specs: specsOfTab(tab) });
    const link = { sessionId: node.id, name: node.name, kind: node.kind, locked: node.locked };
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) => (t.id === tab.id ? { ...t, sessionLink: link, dirty: false } : t)),
      })),
      saveAsPrompt: null,
    }));
    await sessionService.recordOpened(node.kind, node.specs, node.id);
    get().setNotice(`${S.session.savedPrefix}「${node.name}」`);
  },

  /** 会话锁定切换（SES-001；作用于活动标签关联的命名会话） */
  async toggleLockCurrent() {
    const st = get();
    const win = st.windows.find((w) => w.id === st.activeWindowId);
    const tab = win?.tabs.find((t) => t.id === win.activeTabId);
    if (!tab?.sessionLink) {
      get().setNotice(S.session.lockNeedsNamed);
      return;
    }
    const next = !tab.sessionLink.locked;
    await sessionService.setLocked(tab.sessionLink.sessionId, next);
    set((st2) => ({
      windows: st2.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) =>
          t.id === tab.id && t.sessionLink
            ? { ...t, sessionLink: { ...t.sessionLink, locked: next } }
            : t,
        ),
      })),
    }));
    get().setNotice(
      next
        ? `${S.session.lockedOn}「${tab.sessionLink.name}」`
        : `${S.session.lockedOff}「${tab.sessionLink.name}」`,
    );
  },

  /** 会话树右键重命名（SES-001/FR-HOME-002；输入对话框） */
  renameSessionPrompt(sessionId) {
    const node = sessionService.getSession(sessionId);
    if (!node) return;
    get().openPrompt({
      title: S.session.renameTitle,
      initialValue: node.name,
      onSubmit: (value) => {
        if (value === null) return;
        const trimmed = value.trim();
        if (!trimmed) return;
        void sessionService
          .renameSession(sessionId, trimmed)
          .catch((err) => get().setNotice(`${S.session.renameFail}: ${String(err)}`));
      },
    });
  },

  markDirty(tabId, dirty) {
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) => (t.id === tabId ? { ...t, dirty } : t)),
      })),
    }));
  },

  markContentDirty(tabId, sides) {
    const cur = get().findTab(tabId)?.tab.contentDirty;
    if (cur && cur.left === sides.left && cur.right === sides.right) return;
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) => (t.id === tabId ? { ...t, contentDirty: { ...sides } } : t)),
      })),
    }));
  },

  /** 交换左右路径（BC 会话命令；演示脏标记 → 关闭确认 → 保存闭环） */
  swapSides(tabId) {
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) => {
          if (t.id !== tabId || !t.folder) return t;
          const title = `${t.folder.right.split(/[\\/]/).pop()} ⇄ ${t.folder.left.split(/[\\/]/).pop()}`;
          return {
            ...t,
            title,
            folder: { ...t.folder, left: t.folder.right, right: t.folder.left },
            dirty: !!t.sessionLink,
          };
        }),
      })),
    }));
  },

  /**
   * 关闭确认（FR-SES-001 命名会话参数 + R3-05/FR-TXT-020 未落盘文件编辑）。
   *
   * 批量动作（关闭窗口 / 关闭其他 / 退出）经 pendingClose 续跑：确认一次只处理
   * 一个标签，处理完回到批量入口重新扫描，直到没有待守卫项才真正执行。
   */
  async resolveCloseGuard(choice) {
    const guard = get().closeGuard;
    if (!guard) return;
    const { tabId, kind } = guard;
    if (choice === 'cancel') {
      set({ closeGuard: null, pendingClose: null });
      return;
    }
    if (choice === 'save') {
      if (kind === 'content') {
        const landed = await get().saveContentForTab(tabId);
        // 落盘失败/引擎不支持：保留标签与编辑，不静默丢弃（用户可显式选"不保存关闭"）
        if (!landed) {
          set({ closeGuard: null, pendingClose: null });
          return;
        }
        const tab = get().findTab(tabId)?.tab;
        if (tab && closeGuardKind(tab) === 'session') {
          set({ closeGuard: { tabId, kind: 'session' } });
          return;
        }
      } else {
        // 保存失败（如锁定被拒）时按丢弃语义继续关闭：锁定会话本就不保存状态
        await get().saveSessionForTab(tabId);
      }
    }
    set({ closeGuard: null });
    get().markDirty(tabId, false);
    get().markContentDirty(tabId, { left: false, right: false });
    const cont = get().pendingClose;
    if (cont?.kind === 'exit') {
      await get().requestAppExit();
      return;
    }
    if (cont?.kind === 'window') {
      await get().closeWindow(cont.windowId);
      return;
    }
    if (cont?.kind === 'others') {
      await get().closeOthers(cont.tabId);
      return;
    }
    await get().closeTab(tabId);
    set({ closeGuard: null });
  },

  async saveContentForTab(tabId) {
    const tab = get().findTab(tabId)?.tab;
    // D2：独立文本编辑（单文件）——先收敛引擎再落盘，失败保留编辑内容
    if (tab?.kind === 'edit' && tab.edit) {
      const doc = getEditDoc(tab.edit.sessionId);
      if (!doc) {
        get().setNotice(S.edit.loadFail);
        return false;
      }
      const res = await saveEditDoc(doc, { backup: true });
      if (!res.ok) {
        get().setNotice(res.message);
        return false;
      }
      if (res.retargeted) get().retargetEditPath(tabId, res.path);
      get().markContentDirty(tabId, { left: isDocDirty(doc), right: false });
      get().setNotice(savedNotice(res));
      return true;
    }
    const sessionId = tab?.text?.sessionId;
    const dirty = tab?.contentDirty;
    if (!tab || !sessionId || !dirty) return true;
    const out = await runTextSave(sessionId, saveRequestsFor(dirty), { save: engine.saveText });
    applySavedEntries(getTextSession(sessionId), out.saved);
    get().markContentDirty(tabId, remainingSides(dirty, out.saved));
    if (out.unsupported) {
      get().setNotice(S.text.saveUnsupported);
      return false;
    }
    if (out.error !== null) {
      get().setNotice(`${S.text.saveFail}: ${out.error}`);
      return false;
    }
    get().setNotice(`${S.text.saved}：${out.saved.map((e) => e.path).join('、')}`);
    return true;
  },

  async requestAppExit() {
    const pending = firstPendingGuard(get().windows);
    if (!pending) {
      set({ pendingClose: null, closeGuard: null });
      get().exitHandler?.();
      return;
    }
    set({ pendingClose: { kind: 'exit' }, closeGuard: pending });
  },

  setExitHandler(fn) {
    set({ exitHandler: fn });
  },
});

// ------------------------------------------------------------------
// ui slice：状态栏提示 + 对话框编排
// ------------------------------------------------------------------

export const createUiSlice: StateCreator<
  ShellStore,
  [],
  [],
  Pick<
    ShellStore,
    | 'notice'
    | 'setNotice'
    | 'settingsDialog'
    | 'openSettings'
    | 'closeSettings'
    | 'prompt'
    | 'openPrompt'
    | 'resolvePrompt'
    | 'confirm'
    | 'askConfirm'
    | 'resolveConfirm'
    | 'clipboardPanelOpen'
    | 'toggleClipboardPanel'
    | 'reportDialog'
    | 'openReportDialog'
    | 'closeReportDialog'
    | 'toolbarLayout'
    | 'cycleToolbarLayout'
  >
> = (set, get) => ({
  notice: null,
  setNotice(msg) {
    set({ notice: msg });
  },
  settingsDialog: null,
  openSettings(init) {
    set({ settingsDialog: init ?? {} });
  },
  closeSettings() {
    set({ settingsDialog: null });
  },
  prompt: null,
  openPrompt(init) {
    set({ prompt: init });
  },
  resolvePrompt(value) {
    const current = get().prompt;
    set({ prompt: null });
    if (current) current.onSubmit(value);
  },
  confirm: null,
  askConfirm(init) {
    const pending = get().confirm;
    if (pending) pending.onResolve(false);
    return new Promise<boolean>((resolve) => {
      set({ confirm: { ...init, onResolve: resolve } });
    });
  },
  resolveConfirm(ok) {
    const current = get().confirm;
    set({ confirm: null });
    if (current) current.onResolve(ok);
  },
  clipboardPanelOpen: false,
  toggleClipboardPanel(open) {
    set({ clipboardPanelOpen: open ?? !get().clipboardPanelOpen });
  },
  reportDialog: null,
  openReportDialog() {
    const st = get();
    const win = st.windows.find((w) => w.id === st.activeWindowId);
    const tab = win?.tabs.find((t) => t.id === win.activeTabId);
    const sessionId = tab ? sessionIdOfTab(tab) : null;
    if (!tab || !sessionId) {
      st.setNotice(S.report.noSession);
      return;
    }
    set({ reportDialog: { sessionId, kind: tab.kind } });
  },
  closeReportDialog() {
    set({ reportDialog: null });
  },
  toolbarLayout: 'large',
  cycleToolbarLayout() {
    set({ toolbarLayout: nextToolbarLayout(get().toolbarLayout) });
  },
});
