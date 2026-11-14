import { beforeEach, describe, expect, it, vi } from 'vitest';
import { activeWindowSelector, bootstrapShell, resetShellForTest, useShell } from './store';

/**
 * 多窗口/标签行为（FR-WIN-001/002 + SES-001 关闭确认）。
 * 引擎调用（session/close、text/open）在 node 环境下走内置演示引擎路径，
 * 仅涉及会话句柄与幂等关闭，不产生真实 IO。
 */

function setup() {
  resetShellForTest();
  return useShell.getState();
}

describe('shell/store 多窗口与标签', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
  });

  it('bootstrap：主窗口存在且含主页标签', () => {
    bootstrapShell();
    const win = activeWindowSelector(useShell.getState())!;
    expect(win.tabs[0].kind).toBe('home');
    expect(win.activeTabId).toBe(win.tabs[0].id);
  });

  it('openFolderTab：进活动窗口并激活', () => {
    setup();
    useShell.getState().openFolderTab('/a/L', '/a/R');
    const st = useShell.getState();
    const win = activeWindowSelector(st)!;
    expect(win.tabs).toHaveLength(2);
    expect(win.activeTabId).toBe(win.tabs[1].id);
    expect(win.tabs[1].kind).toBe('folder');
    expect(win.tabs[1].title).toBe('L ⇄ R');
  });

  it('detachTab：标签拖出成新窗口（源窗口回退激活，新窗口激活）', () => {
    setup();
    const s0 = useShell.getState();
    s0.openFolderTab('/a/L', '/a/R');
    s0.openFolderTab('/b/L', '/b/R');
    const before = activeWindowSelector(useShell.getState())!;
    const target = before.tabs[1]!;
    useShell.getState().detachTab(target.id);

    const st = useShell.getState();
    expect(st.windows).toHaveLength(2);
    const src = st.windows[0];
    const dst = st.windows[1];
    expect(src.tabs.some((t) => t.id === target.id)).toBe(false);
    expect(dst.tabs.map((t) => t.id)).toEqual([target.id]);
    expect(dst.activeTabId).toBe(target.id);
    expect(st.activeWindowId).toBe(dst.id);
  });

  it('detachTab：主页标签不可拖出', () => {
    setup();
    const win = activeWindowSelector(useShell.getState())!;
    useShell.getState().detachTab(win.tabs[0].id);
    expect(useShell.getState().windows).toHaveLength(1);
  });

  it('mergeAllWindows：其余窗口标签并入主窗口，占位主页丢弃', () => {
    setup();
    useShell.getState().openFolderTab('/a/L', '/a/R');
    const tabId = activeWindowSelector(useShell.getState())!.tabs[1].id;
    useShell.getState().detachTab(tabId); // → 2 窗口
    expect(useShell.getState().windows).toHaveLength(2);
    useShell.getState().activateWindow(useShell.getState().windows[0].id);
    useShell.getState().mergeAllWindows();

    const st = useShell.getState();
    expect(st.windows).toHaveLength(1);
    const kinds = st.windows[0].tabs.map((t) => t.kind);
    expect(kinds.filter((k) => k === 'home')).toHaveLength(1); // 仅保留主窗口主页
    expect(kinds.includes('folder')).toBe(true);
    expect(st.activeWindowId).toBe(st.windows[0].id);
  });

  it('closeOthers：保留指定标签与固定标签，主页不关', async () => {
    setup();
    const s0 = useShell.getState();
    s0.openFolderTab('/a/L', '/a/R');
    s0.openFolderTab('/b/L', '/b/R');
    s0.openFolderTab('/c/L', '/c/R');
    const win = activeWindowSelector(useShell.getState())!;
    const pinnedTab = win.tabs[1]!;
    useShell.getState().togglePin(pinnedTab.id);
    const keep = win.tabs[2]!;
    await useShell.getState().closeOthers(keep.id);

    const after = activeWindowSelector(useShell.getState())!;
    const ids = after.tabs.map((t) => t.id);
    expect(ids).toContain(after.tabs[0].id); // home
    expect(ids).toContain(pinnedTab.id);
    expect(ids).toContain(keep.id);
    expect(ids).toHaveLength(3);
  });

  it('closeTab：普通标签直接关闭并回收；脏的命名会话转关闭确认', async () => {
    setup();
    const s0 = useShell.getState();
    s0.openFolderTab('/a/L', '/a/R');
    const tab = activeWindowSelector(useShell.getState())!.tabs[1]!;
    await useShell.getState().closeTab(tab.id);
    expect(activeWindowSelector(useShell.getState())!.tabs).toHaveLength(1);

    // 命名会话 + 脏 → closeGuard
    const t2tab = {
      id: 'tab-fake',
      kind: 'folder' as const,
      title: 'x',
      folder: { left: '/l', right: '/r', sessionId: 'dir-fake-1' },
      sessionLink: { sessionId: 's-x', name: '命名', kind: 'folder-compare' as const, locked: false },
      dirty: true,
    };
    useShell.setState((st) => ({
      windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, t2tab], activeTabId: t2tab.id })),
    }));
    await useShell.getState().closeTab(t2tab.id);
    expect(useShell.getState().closeGuard?.tabId).toBe(t2tab.id);
    // 取消 → 标签保留
    await useShell.getState().resolveCloseGuard('cancel');
    expect(useShell.getState().closeGuard).toBeNull();
    expect(useShell.getState().findTab(t2tab.id)).toBeDefined();
    // 再次关闭 → 确认框重现；丢弃 → 关闭
    await useShell.getState().closeTab(t2tab.id);
    expect(useShell.getState().closeGuard?.tabId).toBe(t2tab.id);
    await useShell.getState().resolveCloseGuard('discard');
    expect(useShell.getState().findTab(t2tab.id)).toBeUndefined();
    expect(useShell.getState().closeGuard).toBeNull();
  });

  it('togglePin / reorderTab', () => {
    setup();
    const s0 = useShell.getState();
    s0.openFolderTab('/a/L', '/a/R');
    s0.openFolderTab('/b/L', '/b/R');
    const win = activeWindowSelector(useShell.getState())!;
    const t1 = win.tabs[1]!;
    useShell.getState().togglePin(t1.id);
    expect(useShell.getState().findTab(t1.id)!.tab.pinned).toBe(true);
    useShell.getState().togglePin(t1.id);
    expect(useShell.getState().findTab(t1.id)!.tab.pinned).toBe(false);

    const t2 = win.tabs[2]!;
    useShell.getState().reorderTab(t2.id, 1);
    const tabs = activeWindowSelector(useShell.getState())!.tabs;
    expect(tabs[1].id).toBe(t2.id);
  });

  it('openNewWindow / closeWindow：空窗口关闭后回到全新主窗口', async () => {
    setup();
    useShell.getState().openNewWindow();
    expect(useShell.getState().windows).toHaveLength(2);
    const newWin = useShell.getState().windows[1];
    await useShell.getState().closeWindow(newWin.id);
    expect(useShell.getState().windows).toHaveLength(1);

    await useShell.getState().closeWindow(useShell.getState().windows[0].id);
    const st = useShell.getState();
    expect(st.windows).toHaveLength(1);
    expect(st.windows[0].tabs[0].kind).toBe('home');
    expect(st.activeWindowId).toBe(st.windows[0].id);
  });

  it('swapSides：交换路径并标记命名会话脏', () => {
    setup();
    const s0 = useShell.getState();
    s0.openFolderTab('/a/L', '/a/R', {
      sessionLink: { sessionId: 's-1', name: 'n', kind: 'folder-compare', locked: false },
    });
    const tab = activeWindowSelector(useShell.getState())!.tabs[1]!;
    useShell.getState().swapSides(tab.id);
    const after = useShell.getState().findTab(tab.id)!.tab;
    expect(after.folder!.left).toBe('/a/R');
    expect(after.folder!.right).toBe('/a/L');
    expect(after.dirty).toBe(true);
  });

  it('rebindSession：重绑目录/文本会话句柄（resync 降级路径）', () => {
    setup();
    const s0 = useShell.getState();
    s0.openFolderTab('/a/L', '/a/R');
    const tab = activeWindowSelector(useShell.getState())!.tabs[1]!;
    const oldId = tab.folder!.sessionId;
    useShell.getState().rebindSession(oldId, 'dir-new-1');
    expect(useShell.getState().findTab(tab.id)!.tab.folder!.sessionId).toBe('dir-new-1');
  });

  it('activate：跨窗口激活正确窗口内标签', () => {
    setup();
    useShell.getState().openFolderTab('/a/L', '/a/R');
    const tabId = activeWindowSelector(useShell.getState())!.tabs[1].id;
    useShell.getState().detachTab(tabId);
    const homeId = useShell.getState().windows[0].tabs[0].id;
    useShell.getState().activate(homeId);
    const st = useShell.getState();
    expect(st.activeWindowId).toBe(st.windows[0].id);
    expect(st.windows[0].activeTabId).toBe(homeId);
  });

  it('openMerge3Tab：三路路径 + merge3 会话句柄 + 文本合并标题', () => {
    setup();
    useShell.getState().openMerge3Tab('/demo/base.c', '/demo/left.c', '/demo/right.c');
    const win = activeWindowSelector(useShell.getState())!;
    expect(win.tabs).toHaveLength(2);
    const tab = win.tabs[1];
    expect(tab.kind).toBe('merge3');
    expect(tab.merge3?.base).toBe('/demo/base.c');
    expect(tab.merge3?.left).toBe('/demo/left.c');
    expect(tab.merge3?.right).toBe('/demo/right.c');
    expect(tab.merge3?.sessionId).toMatch(/^merge3-/);
    expect(tab.title).toContain('文本合并');
    expect(win.activeTabId).toBe(tab.id);
  });
});
