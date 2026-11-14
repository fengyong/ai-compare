import { create, type StoreApi, type UseBoundStore } from 'zustand';
import type { ShellStore } from './shellState';
import { createTabsSlice, createWindowsSlice, makeWindow } from './sliceWindowsTabs';
import { createSessionSlice, createUiSlice } from './sliceSessionUi';

/**
 * 应用壳状态（M30）——P4 起多窗口化（FR-WIN-001/002）：
 * zustand slice 组合（窗口 / 标签 / 会话 / UI 四 slice，见 sliceWindowsTabs.ts、
 * sliceSessionUi.ts），大数据（目录行缓存、文本行）仍留在各视图的非响应式缓存。
 *
 * 兼容说明：P0 的单列表 tabs/activeTabId 已升级为 windows[]；
 * 消费方用 useShell((st) => st.windows) / activeWindowSelector 取活动窗口。
 */

export type { ShellStore, Tab, WindowState, SessionLink, SettingsDialogState } from './shellState';
export { nextTabId } from './sliceWindowsTabs';

/** 活动窗口选择器（窗口缺失时兜底到第一个；空 store 由 bootstrap 保证不出现） */
export function activeWindowSelector(st: ShellStore) {
  return st.windows.find((w) => w.id === st.activeWindowId) ?? st.windows[0];
}

/** 组合 store（windows → tabs → sessions → ui） */
export const useShell: UseBoundStore<StoreApi<ShellStore>> = create<ShellStore>()((...a) => ({
  ...createWindowsSlice(...a),
  ...createTabsSlice(...a),
  ...createSessionSlice(...a),
  ...createUiSlice(...a),
}));

/** 应用启动：保证存在主窗口（主页为首个标签，FR-WIN-001/SET-005） */
export function bootstrapShell(): void {
  const st = useShell.getState();
  if (st.windows.length === 0) {
    useShell.setState({ windows: [makeWindow('w-1-main', true)], activeWindowId: 'w-1-main' });
  }
}

// 模块加载即引导主窗口（AppShell/main 无需显式调用；测试用 resetShellForTest 复位）
bootstrapShell();

/** 测试辅助：复位到全新主窗口（含关闭守卫编排状态，避免跨用例串味） */
export function resetShellForTest(): void {
  useShell.setState({
    windows: [makeWindow('w-1-main', true)],
    activeWindowId: 'w-1-main',
    closeGuard: null,
    pendingClose: null,
    exitHandler: null,
  });
}

// 兼容旧引用：FolderCompareView / HomeView 等仍以 useShell 为主入口
export { useShell as shellStore };
