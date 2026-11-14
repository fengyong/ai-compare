import type { StateCreator } from 'zustand';
import { sessionService } from '../session/service';
import type { SessionKind } from '../session/model';
import { S } from '../strings';
import type { SessionSlice, ShellStore, Tab } from './shellState';

/** 会话 slice（SES-001~006）：打开已保存会话 / 保存 / 另存 / 锁定 / 关闭确认 */
export const createSessionSlice: StateCreator<ShellStore, [], [], SessionSlice> = (set, get) => ({
  closeGuard: null,
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
      get().setNotice(`${S.session.lockedNoticePrefix}「${node.name}」${S.session.lockedNoticeSuffix}`);
    }
    const link = { sessionId: node.id, name: node.name, kind: node.kind, locked: node.locked };
    if (node.kind === 'folder-compare') {
      get().openFolderTab(node.specs.left, node.specs.right, { sessionLink: link });
    } else if (node.kind === 'text-compare') {
      await get().openTextTab(node.specs.left, node.specs.right, { sessionLink: link });
    } else if (node.kind === 'folder-sync') {
      get().openFolderSyncTab(node.specs.left, node.specs.right, { sessionLink: link });
    } else if (node.kind === 'folder-merge') {
      // 三路会话以 left 作为 base 的回退（保存的 specs 只有 left/right 两个路径位）
      get().openFolderMergeTab(node.specs.left, node.specs.left, node.specs.right, { sessionLink: link });
    } else {
      get().setNotice(S.session.kindNotImplemented);
      return;
    }
    await sessionService.recordOpened(node.kind, node.specs, node.id);
  },

  /** 命名会话保存（SES-001：显式保存；未命名标签转另存流程） */
  async saveSessionForTab(tabId) {
    const found = get().findTab(tabId);
    if (!found) return;
    const tab = found.tab;
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
        tab.sessionLink.sessionId
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
    const kind: SessionKind =
      tab.kind === 'text'
        ? 'text-compare'
        : tab.kind === 'sync'
          ? 'folder-sync'
          : tab.kind === 'merge'
            ? 'folder-merge'
            : tab.kind === 'merge3'
              ? 'text-merge'
              : 'folder-compare';
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
          t.id === tab.id && t.sessionLink ? { ...t, sessionLink: { ...t.sessionLink, locked: next } } : t
        ),
      })),
    }));
    get().setNotice(next ? `${S.session.lockedOn}「${tab.sessionLink.name}」` : `${S.session.lockedOff}「${tab.sessionLink.name}」`);
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

  /** 关闭确认（FR-SES-001：修改命名会话后退出提示保存） */
  async resolveCloseGuard(choice) {
    const guard = get().closeGuard;
    if (!guard) return;
    const { tabId } = guard;
    if (choice === 'cancel') {
      set({ closeGuard: null });
      return;
    }
    if (choice === 'save') {
      // 保存失败（如锁定被拒）时按丢弃语义继续关闭：锁定会话本就不保存状态
      await get().saveSessionForTab(tabId);
    }
    set({ closeGuard: null });
    get().markDirty(tabId, false);
    await get().closeTab(tabId);
    set({ closeGuard: null });
  },
});

/** 标签 → 会话规格（保存用；三路合并会话 base 落 left 位、right 落 right 位） */
function specsOfTab(tab: Tab): { left: string; right: string } {
  if (tab.kind === 'folder' && tab.folder) return { left: tab.folder.left, right: tab.folder.right };
  if (tab.kind === 'sync' && tab.sync) return { left: tab.sync.left, right: tab.sync.right };
  if (tab.kind === 'merge' && tab.merge) return { left: tab.merge.base, right: tab.merge.right };
  if (tab.kind === 'merge3' && tab.merge3) return { left: tab.merge3.base, right: tab.merge3.right };
  if (tab.kind === 'text' && tab.text) return { left: tab.text.left.path, right: tab.text.right.path };
  return { left: '', right: '' };
}

// ------------------------------------------------------------------
// ui slice：状态栏提示 + 对话框编排
// ------------------------------------------------------------------

export const createUiSlice: StateCreator<
  ShellStore,
  [],
  [],
  Pick<ShellStore, 'notice' | 'setNotice' | 'settingsDialog' | 'openSettings' | 'closeSettings' | 'prompt' | 'openPrompt' | 'resolvePrompt'>
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
});
