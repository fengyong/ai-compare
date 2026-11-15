import type { StateCreator } from 'zustand';
import * as engine from '../ipc/engineProcess';
import { getDirCache, releaseDirCache } from '../views/folder-compare/dirRowCache';
import { buildOpsForCommand } from '../views/folder-compare/dirOps';
import {
  currentDiffIndex,
  orderedDiffFileRows,
  pickDiffTarget,
} from '../views/text-compare/diffFileNav';
import { parentRescanParams } from '../views/text-compare/parentWriteback';
import { deleteTextSession } from '../views/text-compare/textModel';
import { basename, joinPath } from '../lib/env';
import { S } from '../strings';
import { closeGuardKind, firstPendingGuard, guardOf } from './closeGuard';
import { resolveViewKind } from './settings/formatsModel';
import { effectiveFormatsFromSettings } from './settings/formatsRuntime';
import { effectiveRulesFor } from '../views/text-compare/rulesRuntime';
import type { ShellStore, Tab, WindowState, WindowsSlice, TabsSlice, OpenTabOpts } from './shellState';

/** 窗口 slice（FR-WIN-001/002）：多窗口、拖出标签、合并窗口 */
export const createWindowsSlice: StateCreator<ShellStore, [], [], WindowsSlice> = (set, get) => ({
  windows: [],
  activeWindowId: '',

  openNewWindow(opts) {
    const win = makeWindow(`w-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6)}`, opts?.withHomeTab !== false);
    set((st) => ({ windows: [...st.windows, win], activeWindowId: win.id }));
  },

  activateWindow(windowId) {
    set({ activeWindowId: windowId });
  },

  /** 标签拖出成新窗口（FR-WIN-001"移动标签页到新窗口"；home 标签不可拖出） */
  detachTab(tabId) {
    const st = get();
    const found = st.findTab(tabId);
    if (!found || found.tab.kind === 'home') return;
    const src = st.windows.find((w) => w.id === found.windowId)!;
    const remaining = src.tabs.filter((t) => t.id !== tabId);
    const idx = src.tabs.findIndex((t) => t.id === tabId);
    const srcActive =
      src.activeTabId === tabId ? remaining[Math.max(0, idx - 1)]?.id ?? remaining[0]?.id ?? '' : src.activeTabId;
    const win = makeWindow(`w-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6)}`, false);
    win.tabs = [found.tab];
    win.activeTabId = tabId;
    win.title = found.tab.title;
    set(() => ({
      windows: st.windows.map((w) =>
        w.id === src.id ? { ...w, tabs: remaining, activeTabId: srcActive, title: titleOf(srcActive, remaining) } : w
      ).concat([win]),
      activeWindowId: win.id,
    }));
  },

  /** 合并所有窗口到活动窗口（FR-WIN-001"合并所有窗口"；其余窗口的占位主页丢弃） */
  mergeAllWindows() {
    const st = get();
    const target = st.windows.find((w) => w.id === st.activeWindowId) ?? st.windows[0];
    if (!target) return;
    const others = st.windows.filter((w) => w.id !== target.id);
    if (others.length === 0) return;
    const absorbed = others.flatMap((w) => w.tabs.filter((t) => t.kind !== 'home'));
    set(() => ({
      windows: st.windows
        .filter((w) => w.id === target.id)
        .map((w) => ({ ...w, tabs: [...w.tabs.filter((t) => t.kind === 'home'), ...absorbed] })),
      activeWindowId: target.id,
    }));
  },

  async closeWindow(windowId) {
    const win = get().windows.find((w) => w.id === windowId);
    if (!win) return;
    // 脏标签（未落盘编辑 / 命名会话修改）逐个确认；否则下方整窗移除会
    // 静默丢弃未保存内容（FR-SES-001 / R3-05）。
    const pending = firstPendingGuard([win]);
    if (pending) {
      set({ closeGuard: pending, pendingClose: { kind: 'window', windowId } });
      return;
    }
    set({ pendingClose: null });
    for (const t of [...win.tabs]) {
      if (t.kind !== 'home') await get().closeTab(t.id);
    }
    set((st) => {
      let windows = st.windows.filter((w) => w.id !== windowId);
      if (windows.length === 0) {
        const fresh = makeWindow('w-1-main', true);
        windows = [fresh];
        return { windows, activeWindowId: fresh.id };
      }
      const activeWindowId = st.activeWindowId === windowId ? windows[0].id : st.activeWindowId;
      return { windows, activeWindowId };
    });
  },

  windowCount() {
    return get().windows.length;
  },
});

// ------------------------------------------------------------------
// tabs slice（FR-WIN-001 标签页 + P0 子会话闭环）
// ------------------------------------------------------------------

let tabSeq = 1;
export const nextTabId = (kind: string): string => `tab-${kind}-${tabSeq++}`;

export function makeWindow(id: string, withHomeTab: boolean): WindowState {
  const home: Tab = { id: `home-${id}`, kind: 'home', title: S.app.homeTab };
  return withHomeTab
    ? { id, title: home.title, tabs: [home], activeTabId: home.id }
    : { id, title: '', tabs: [], activeTabId: '' };
}

export function titleOf(tabId: string | undefined, tabs: Tab[]): string {
  return tabs.find((t) => t.id === tabId)?.title ?? S.app.homeTab;
}

export const createTabsSlice: StateCreator<ShellStore, [], [], TabsSlice> = (set, get) => ({
  /** 队列 3：三向文本合并标签（FR-TMRG-001：base=公共祖先 + left/right 两侧） */
  openMerge3Tab(base, left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('merge3');
    const title = `${S.tab.merge3TitleSuffix}：${basename(left)} + ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'merge3',
      title,
      merge3: { base, left, right, sessionId: engine.newSessionId('merge3') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? { ...w, tabs: [...w.tabs, tab], activeTabId: activate ? id : w.activeTabId, title: activate ? title : w.title }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },
  /** 文件夹同步标签（P8，FR-SYNC-001）：复用目录会话句柄，视图承载同步方法/策略/计划/报告 */
  /** P14：十六进制比较标签（FR-HEX）：两侧二进制路径 + hex/open 会话句柄 */
  openHexTab(left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('hex');
    const title = `${S.tab.hexTitleSuffix}：${basename(left)} ⇄ ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'hex',
      title,
      hex: { left, right, sessionId: engine.newSessionId('hex') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? { ...w, tabs: [...w.tabs, tab], activeTabId: activate ? id : w.activeTabId, title: activate ? title : w.title }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },
  /** P14：表格比较标签（FR-TBL）：两侧 CSV 路径 + table/open 会话句柄 */
  openTableTab(left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('table');
    const title = `${S.tab.tableTitleSuffix}：${basename(left)} ⇄ ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'table',
      title,
      table: { left, right, sessionId: engine.newSessionId('table') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? { ...w, tabs: [...w.tabs, tab], activeTabId: activate ? id : w.activeTabId, title: activate ? title : w.title }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },
  /** P14：图片比较标签（FR-PIC）：两侧图片路径 + picture/open 会话句柄 */
  openPictureTab(left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('picture');
    const title = `${S.tab.pictureTitleSuffix}：${basename(left)} ⇄ ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'picture',
      title,
      picture: { left, right, sessionId: engine.newSessionId('picture') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? { ...w, tabs: [...w.tabs, tab], activeTabId: activate ? id : w.activeTabId, title: activate ? title : w.title }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },
  openFolderSyncTab(left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('sync');
    const title = `${S.tab.syncTitleSuffix}：${basename(left)} ⇄ ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'sync',
      title,
      sync: { left, right, sessionId: engine.newSessionId('dir') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? { ...w, tabs: [...w.tabs, tab], activeTabId: activate ? id : w.activeTabId, title: activate ? title : w.title }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },

  /** 文件夹合并标签（P8，FR-DMRG-001）：三路 base/left/right */
  openFolderMergeTab(base, left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('merge');
    const title = `${S.tab.mergeTitleSuffix}：${basename(left)} + ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'merge',
      title,
      merge: { base, left, right, sessionId: engine.newSessionId('merge') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? { ...w, tabs: [...w.tabs, tab], activeTabId: activate ? id : w.activeTabId, title: activate ? title : w.title }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },

  openFolderTab(left, right, opts: OpenTabOpts = {}) {
    const id = nextTabId('dir');
    const title = `${basename(left)} ⇄ ${basename(right)}`;
    const tab: Tab = {
      id,
      kind: 'folder',
      title,
      folder: { left, right, sessionId: engine.newSessionId('dir') },
      sessionLink: opts.sessionLink,
    };
    set((st) => {
      const targetId = opts.windowId ?? st.activeWindowId;
      const activate = opts.activate !== false;
      return {
        windows: st.windows.map((w) =>
          w.id === targetId
            ? {
                ...w,
                tabs: [...w.tabs, tab],
                activeTabId: activate ? id : w.activeTabId,
                title: activate ? title : w.title,
              }
            : w
        ),
        activeWindowId: opts.windowId ? targetId : st.activeWindowId,
      };
    });
  },

  /** 独立文本比较（主页"文本比较"大按钮入口） */
  async openTextTab(leftPath, rightPath, opts: OpenTabOpts = {}) {
    try {
      const sessionId = engine.newSessionId('text');
      await engine.openText({
        sessionId,
        left: leftPath,
        right: rightPath,
        options: null,
        rules: effectiveRulesFor(leftPath, rightPath),
      });
      const id = nextTabId('text');
      const tab: Tab = {
        id,
        kind: 'text',
        title: `${basename(leftPath)}（${S.tab.textTitleSuffix}）`,
        text: {
          sessionId,
          left: { path: leftPath, name: basename(leftPath) },
          right: { path: rightPath, name: basename(rightPath) },
        },
        sessionLink: opts.sessionLink,
      };
      set((st) => {
        const targetId = opts.windowId ?? st.activeWindowId;
        return {
          windows: st.windows.map((w) =>
            w.id === targetId ? { ...w, tabs: [...w.tabs, tab], activeTabId: id, title: tab.title } : w
          ),
        };
      });
    } catch (err) {
      get().setNotice(`${S.folder.openFail}: ${err instanceof Error ? err.message : String(err)}`);
    }
  },

  /**
   * 双击目录行 → 按文件格式列表决定的视图类型打开子会话（FR-DIR-070 / FR-SEL-001）。
   *
   * 地址必须取 leftPath/rightPath：树形模式 id 恰好等于 rel，但平铺模式 id 是
   * `name` / `name#i`（显示消歧），手动对齐时左右还可能不同名（R3-03）。
   * 文本子会话记录父目录链接（保存回传 FR-SES-004、差异文件遍历 FR-DIR-071）；
   * 表格/十六进制/图片视图当前只读，无回传与遍历需求，故不记父链接。
   */
  async openChild(parentTabId, row) {
    const found = get().findTab(parentTabId);
    const folder = found?.tab.folder;
    if (!found || !folder) return;
    const leftRel = row.leftPath;
    const rightRel = row.rightPath;
    if (!leftRel || !rightRel) {
      get().setNotice(S.text.unsupportedView);
      return;
    }
    const left = joinPath(folder.left, leftRel);
    const right = joinPath(folder.right, rightRel);
    const kind = resolveViewKind(effectiveFormatsFromSettings(), row.name);
    if (kind === 'external') {
      get().setNotice(S.text.externalView);
      return;
    }
    if (kind !== 'text') {
      const opts: OpenTabOpts = { windowId: found.windowId };
      if (kind === 'table') get().openTableTab(left, right, opts);
      else if (kind === 'hex') get().openHexTab(left, right, opts);
      else get().openPictureTab(left, right, opts);
      return;
    }
    const baseName = (rel: string): string => rel.split('/').pop() || rel;
    try {
      const sessionId = engine.newSessionId('text');
      await engine.openText({
        sessionId,
        left,
        right,
        options: null,
        rules: effectiveRulesFor(left, right),
      });
      const id = nextTabId('text');
      const tab: Tab = {
        id,
        kind: 'text',
        title: `${row.name}（${S.tab.textTitleSuffix}）`,
        text: {
          sessionId,
          left: { path: left, name: baseName(leftRel) },
          right: { path: right, name: baseName(rightRel) },
          parentTabId,
          parentFolderSessionId: folder.sessionId,
        },
      };
      set((st) => ({
        windows: st.windows.map((w) =>
          w.id === found.windowId ? { ...w, tabs: [...w.tabs, tab], activeTabId: id, title: tab.title } : w
        ),
        activeWindowId: found.windowId,
      }));
    } catch (err) {
      get().setNotice(`${S.folder.openFail}: ${err instanceof Error ? err.message : String(err)}`);
    }
  },

  /**
   * 子会话差异文件遍历（FR-DIR-071）：跳到父目录会话的下一/上一差异文件对，
   * 可先整文件复制到对侧（BC"复制文件到右侧并打开下一个差异"）。
   *
   * 目标在复制之前算定：复制 + 回传后当前对会判为 same 并从差异列表消失，
   * 那时再算就丢了位置。
   */
  async navigateDiffFile(tabId, dir, copyTo) {
    const st = get();
    const found = st.findTab(tabId);
    const text = found?.tab.text;
    if (!found || !text || !text.parentTabId || !text.parentFolderSessionId) return;
    const parentSessionId = text.parentFolderSessionId;
    const parent = st.findTab(text.parentTabId)?.tab.folder;
    if (!parent) return;
    // 跳转等于丢弃当前会话：未落盘编辑必须先经守卫（与关闭标签同一口径，R3-05）
    if (closeGuardKind(found.tab)) {
      st.setNotice(S.text.navDirty);
      return;
    }
    const rows = getDirCache(parentSessionId).getRows();
    const ordered = orderedDiffFileRows(rows);
    const current = { leftPath: text.left.path, rightPath: text.right.path };
    const index = currentDiffIndex(ordered, current, parent);
    const target = pickDiffTarget(ordered, index, dir);
    if (!target) {
      st.setNotice(S.text.noDiffFiles);
      return;
    }
    if (copyTo) {
      const rowIndex = currentDiffIndex(rows, current, parent);
      const row = rowIndex >= 0 ? rows[rowIndex] : null;
      if (row) {
        const ok = await st.askConfirm({
          title: S.text.navCopyTitle,
          message: S.text.navCopyMessage(
            row.name,
            copyTo === 'right' ? S.text.paneRight : S.text.paneLeft,
          ),
          confirmLabel: S.text.navCopyOk,
        });
        if (!ok) return;
        const ops = buildOpsForCommand(
          copyTo === 'right' ? 'copyToRight' : 'copyToLeft',
          [row],
          new Set([row.id]),
        );
        try {
          const result = await engine.executeOperations({ sessionId: parentSessionId, ops });
          if (result.failed.length > 0) {
            st.setNotice(S.text.navCopyFailed(result.failed[0].detail ?? ''));
            return;
          }
        } catch (err) {
          st.setNotice(S.text.navCopyFailed(err instanceof Error ? err.message : String(err)));
          return;
        }
        const rescan = parentRescanParams(parentSessionId);
        if (rescan) void engine.filterDir(rescan).catch(() => undefined);
      }
    }
    const left = joinPath(parent.left, target.leftPath);
    const right = joinPath(parent.right, target.rightPath);
    if (left === current.leftPath && right === current.rightPath) {
      st.setNotice(S.text.noMoreDiffFiles);
      return;
    }
    const sessionId = engine.newSessionId('text');
    try {
      await engine.openText({
        sessionId,
        left,
        right,
        options: null,
        rules: effectiveRulesFor(left, right),
      });
    } catch (err) {
      st.setNotice(`${S.folder.openFail}: ${err instanceof Error ? err.message : String(err)}`);
      return;
    }
    // 旧会话按关闭标签的同一套清理（句柄注销 + 模型缓存释放 + 幂等 close）
    engine.untrackSession(text.sessionId);
    deleteTextSession(text.sessionId);
    void engine.closeSession({ sessionId: text.sessionId }).catch(() => undefined);
    const baseName = (rel: string): string => rel.split('/').pop() || rel;
    const title = `${target.name}（${S.tab.textTitleSuffix}）`;
    set((state) => ({
      windows: state.windows.map((w) => {
        const tabs = w.tabs.map((t) =>
          t.id === tabId && t.text
            ? {
                ...t,
                title,
                text: {
                  ...t.text,
                  sessionId,
                  left: { path: left, name: baseName(target.leftPath) },
                  right: { path: right, name: baseName(target.rightPath) },
                },
              }
            : t,
        );
        return { ...w, tabs, title: titleOf(w.activeTabId, tabs) };
      }),
    }));
  },

  activate(tabId) {
    set((st) => {
      let focusWindowId: string | null = null;
      const windows = st.windows.map((w) => {
        const tab = w.tabs.find((t) => t.id === tabId);
        if (!tab) return w;
        focusWindowId = w.id;
        return { ...w, activeTabId: tabId, title: tab.title };
      });
      return focusWindowId ? { windows, activeWindowId: focusWindowId } : { windows };
    });
  },

  retargetTextSide(tabId, side, path) {
    set((st) => ({
      windows: st.windows.map((w) => {
        const tabs = w.tabs.map((t) => {
          if (t.id !== tabId || !t.text) return t;
          const text = { ...t.text, [side]: { path, name: basename(path) } };
          return { ...t, text, title: `${basename(text.left.path)}（${S.tab.textTitleSuffix}）` };
        });
        return { ...w, tabs, title: titleOf(w.activeTabId, tabs) };
      }),
    }));
  },

  setMerge3OutputPath(tabId, path) {
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) =>
          t.id === tabId && t.merge3 ? { ...t, merge3: { ...t.merge3, outputPath: path } } : t,
        ),
      })),
    }));
  },

  /** 关闭（未落盘编辑或命名会话有未保存修改时转 closeGuard；引擎会话幂等清理） */
  async closeTab(tabId) {
    const found = get().findTab(tabId);
    if (!found || found.tab.kind === 'home') return;
    const pending = guardOf(found.tab);
    if (pending) {
      set({ closeGuard: pending });
      return;
    }
    await cleanupTabEngines(found.tab);
    set((st) => ({
      windows: st.windows.map((w) => {
        if (w.id !== found.windowId) return w;
        const idx = w.tabs.findIndex((t) => t.id === tabId);
        const tabs = w.tabs.filter((t) => t.id !== tabId);
        const activeTabId =
          w.activeTabId === tabId ? tabs[Math.max(0, idx - 1)]?.id ?? tabs[0]?.id ?? '' : w.activeTabId;
        return { ...w, tabs, activeTabId, title: titleOf(activeTabId, tabs) };
      }),
    }));
  },

  /** 关闭其他标签（保留指定标签与固定标签；home 占位不关） */
  async closeOthers(tabId) {
    const found = get().findTab(tabId);
    if (!found) return;
    const win = get().windows.find((w) => w.id === found.windowId)!;
    // 脏标签逐个确认：否则下方批量过滤会静默丢弃未保存内容（FR-SES-001 / R3-05）。
    const closing: WindowState = {
      ...win,
      tabs: win.tabs.filter((t) => t.id !== tabId && !t.pinned),
    };
    const pending = firstPendingGuard([closing]);
    if (pending) {
      set({ closeGuard: pending, pendingClose: { kind: 'others', tabId } });
      return;
    }
    set({ pendingClose: null });
    for (const t of win.tabs) {
      if (t.kind !== 'home' && t.id !== tabId && !t.pinned) await cleanupTabEngines(t);
    }
    set((st) => ({
      windows: st.windows.map((w) => {
        if (w.id !== found.windowId) return w;
        const tabs = w.tabs.filter((t) => t.kind === 'home' || t.id === tabId || t.pinned);
        return { ...w, tabs, activeTabId: tabId, title: found.tab.title };
      }),
    }));
  },

  togglePin(tabId) {
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) => (t.id === tabId ? { ...t, pinned: !t.pinned } : t)),
      })),
    }));
  },

  /** 标签在窗口内拖拽重排（HTML5 DnD 的落点索引） */
  reorderTab(tabId, toIndex) {
    set((st) => ({
      windows: st.windows.map((w) => {
        const from = w.tabs.findIndex((t) => t.id === tabId);
        if (from < 0) return w;
        const tabs = [...w.tabs];
        const [moved] = tabs.splice(from, 1);
        const dest = toIndex > from ? toIndex - 1 : toIndex;
        tabs.splice(Math.max(0, Math.min(dest, tabs.length)), 0, moved);
        return { ...w, tabs };
      }),
    }));
  },

  /** 引擎重连后重绑会话句柄（resync 降级路径：UI 侧全量重放 delta） */
  rebindSession(oldSessionId, newSessionId) {
    releaseDirCache(oldSessionId);
    set((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) => rebindTabSession(t, oldSessionId, newSessionId)),
      })),
    }));
  },

  findTab(tabId) {
    for (const w of get().windows) {
      const tab = w.tabs.find((t) => t.id === tabId);
      if (tab) return { tab, windowId: w.id };
    }
    return undefined;
  },
});

/**
 * 标签上持有引擎会话句柄的子视图种类（全覆盖清单）。
 *
 * 背景：引擎侧 sessions map 为 picture/table/hex/text/merge/merge3/dir 全部建会话，
 * 此前清理只覆盖 text/folder/merge3 → 其余类型的会话在引擎侧只增不减（内存泄漏），
 * 且崩溃重放后会拿旧句柄继续请求（410/411）。
 */
const TAB_SESSION_KINDS = ['text', 'folder', 'sync', 'picture', 'table', 'hex', 'merge', 'merge3'] as const;

/** 取标签上首个非空的引擎会话句柄（同一标签通常只有一个视图持有会话） */
export function tabSessionId(tab: Tab): string | undefined {
  for (const kind of TAB_SESSION_KINDS) {
    const holder = tab[kind] as { sessionId?: string } | undefined;
    if (holder?.sessionId) return holder.sessionId;
  }
  return undefined;
}

/** 引擎重连后重绑标签的会话句柄（覆盖全部视图种类） */
function rebindTabSession(tab: Tab, oldSessionId: string, newSessionId: string): Tab {
  let changed = false;
  const next: Tab = { ...tab };
  const mutable = next as unknown as Record<string, unknown>;
  for (const kind of TAB_SESSION_KINDS) {
    const holder = tab[kind] as { sessionId?: string } | undefined;
    if (holder?.sessionId === oldSessionId) {
      mutable[kind] = { ...holder, sessionId: newSessionId };
      changed = true;
    }
  }
  return changed ? next : tab;
}

/** 关闭标签的引擎侧清理（session/close 幂等 + 目录缓存释放 + 崩溃恢复登记注销） */
export async function cleanupTabEngines(tab: Tab): Promise<void> {
  const sessionId = tabSessionId(tab);
  if (!sessionId) return;
  engine.untrackSession(sessionId);
  try {
    await engine.closeSession({ sessionId });
  } catch (err) {
    console.warn('[session/close] 失败', err);
  }
  // 目录视图另有 UI 侧行缓存需要释放
  if (tab.folder?.sessionId) {
    releaseDirCache(tab.folder.sessionId);
  }
  // 文本视图的会话模型缓存（编辑内容/撤销栈）随标签关闭一并释放：
  // 否则关闭后仍留有陈旧模型，且缓存只增不减（R3-05）
  if (tab.text?.sessionId) {
    deleteTextSession(tab.text.sessionId);
  }
}
