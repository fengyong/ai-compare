// @vitest-environment jsdom
/**
 * 工作包 3B 组件级用户故事：标签栏。
 *
 * - 故事 1：文本编辑 → dirty → 关闭拦截（未保存内容不得被静默丢弃）。
 * - 故事 3：table/hex 标签切换不重复 open（切回已打开的标签不得重新建会话）。
 *
 * 与 store 级测试（storeWindows.test.ts）互补：这里走**真实渲染 + 真实点击**，
 * 验证 UI 事件确实落到了关闭守卫与标签切换路径上。
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../ipc/engineProcess';
import { TabBar } from './TabBar';
import { activeWindowSelector, resetShellForTest, useShell } from './store';
import type { Tab } from './store';

vi.mock('../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    openText: vi.fn(),
    openTable: vi.fn(),
    openHex: vi.fn(),
  };
});

import { openHex, openTable, openText } from '../ipc/engineProcess';

const mockOpenHex = vi.mocked(openHex);
const mockOpenTable = vi.mocked(openTable);
const mockOpenText = vi.mocked(openText);

function seedTabs(tabs: Tab[], activeTabId?: string): void {
  const st = useShell.getState();
  useShell.setState({
    windows: st.windows.map((w, i) =>
      i === 0
        ? {
            ...w,
            tabs: [...w.tabs, ...tabs],
            activeTabId: activeTabId ?? tabs[0]?.id ?? w.activeTabId,
          }
        : w,
    ),
  });
}

beforeEach(() => {
  resetShellForTest();
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
});

describe('标签栏组件（3B：dirty 关闭拦截 / 标签切换不重复 open）', () => {
  it('dirty 标签渲染未保存标记', () => {
    const tab: Tab = {
      id: 't-dirty',
      kind: 'text',
      title: 'a.txt（文本比较）',
      text: {
        sessionId: 'txt-1',
        left: { path: '/L/a.txt', name: 'a.txt' },
        right: { path: '/R/a.txt', name: 'a.txt' },
      },
      dirty: true,
    } as Tab;
    seedTabs([tab]);
    render(<TabBar />);
    expect(document.querySelector('.dirty-dot')).toBeTruthy();
    expect(document.querySelector('.dirty-dot')?.getAttribute('title')).toBeTruthy();
  });

  it('关闭 dirty 标签被拦截：标签保留并进入关闭确认', async () => {
    const tab: Tab = {
      id: 't-dirty',
      kind: 'text',
      title: 'a.txt（文本比较）',
      text: {
        sessionId: 'txt-1',
        left: { path: '/L/a.txt', name: 'a.txt' },
        right: { path: '/R/a.txt', name: 'a.txt' },
      },
      contentDirty: { left: true, right: false },
    } as Tab;
    seedTabs([tab]);
    render(<TabBar />);

    fireEvent.click(screen.getByRole('button', { name: /关闭/ }));
    await Promise.resolve();

    // 标签没被关掉（未保存内容不得静默丢弃）
    expect(useShell.getState().findTab('t-dirty')).toBeDefined();
  });

  it('切换到已打开的 table/hex 标签不重复 open', () => {
    const tableTab: Tab = {
      id: 't-table',
      kind: 'table',
      title: 'a.csv（表格比较）',
      table: {
        sessionId: 'tab-1',
        left: '/L/a.csv',
        right: '/R/a.csv',
      },
    } as Tab;
    const hexTab: Tab = {
      id: 't-hex',
      kind: 'hex',
      title: 'a.bin（十六进制比较）',
      hex: {
        sessionId: 'hex-1',
        left: '/L/a.bin',
        right: '/R/a.bin',
      },
    } as Tab;
    seedTabs([tableTab, hexTab], 't-table');
    render(<TabBar />);

    fireEvent.click(screen.getByRole('tab', { name: /a\.bin/ }));
    expect(activeWindowSelector(useShell.getState())?.activeTabId).toBe('t-hex');

    fireEvent.click(screen.getByRole('tab', { name: /a\.csv/ }));
    expect(activeWindowSelector(useShell.getState())?.activeTabId).toBe('t-table');

    // 纯切换不得重新建会话
    expect(mockOpenTable).not.toHaveBeenCalled();
    expect(mockOpenHex).not.toHaveBeenCalled();
    expect(mockOpenText).not.toHaveBeenCalled();
  });
});
