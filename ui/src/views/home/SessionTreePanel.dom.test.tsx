// @vitest-environment jsdom
/**
 * 工作包 3B 组件级用户故事：会话树（保存 → 树中可见 → 打开恢复）。
 *
 * 存取本身由 session/service.test.ts 覆盖（保存 → 持久化 → 重新 init 读回）；
 * 这里覆盖**面板层**：保存的会话出现在树里、双击触发打开入口。
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../../session/service';
import { SessionTreePanel } from './SessionTreePanel';
import { resetShellForTest, useShell } from '../../shell/store';

vi.mock('../../session/service', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    sessionService: {
      ...actual.sessionService,
      getTree: vi.fn(),
      getMruSessions: vi.fn(() => [] as string[]),
      subscribe: vi.fn(() => () => undefined),
      deleteSession: vi.fn(),
      setLocked: vi.fn(),
      deleteFolder: vi.fn(),
    },
  };
});

import { sessionService } from '../../session/service';
import type { SessionTreeVM } from '../../session/service';

const getTree = vi.mocked(sessionService.getTree);

/** 树数据：一个已保存的文件夹对比会话 */
function treeWithSession(): SessionTreeVM {
  return {
    rootFolderIds: [],
    folders: new Map(),
    sessions: new Map([
      [
        's-1',
        {
          id: 's-1',
          kind: 'folder-compare',
          name: '发布目录对比',
          locked: false,
          createdAt: '2026-01-01T00:00:00Z',
          updatedAt: '2026-01-01T00:00:00Z',
          folderId: null,
          overrides: {},
          state: {},
          specs: { left: '/tmp/L', right: '/tmp/R' },
        },
      ],
    ]),
  };
}

beforeEach(() => {
  resetShellForTest();
  vi.clearAllMocks();
  getTree.mockReturnValue(treeWithSession());
});

afterEach(() => {
  cleanup();
});

describe('会话树面板（3B：各会话保存恢复）', () => {
  it('保存过的会话出现在树里，并带上左右路径提示', async () => {
    render(<SessionTreePanel />);
    await waitFor(() => expect(screen.getByText('发布目录对比')).toBeTruthy());
    const row = document.querySelector('.aic-tree-row.session');
    expect(row?.getAttribute('title')).toContain('/tmp/L ⇄ /tmp/R');
  });

  it('双击已保存会话触发打开入口（恢复路径的 UI 起点）', async () => {
    const openSavedSession = vi.fn();
    useShell.setState({ openSavedSession } as never);
    render(<SessionTreePanel />);
    await waitFor(() => expect(screen.getByText('发布目录对比')).toBeTruthy());
    fireEvent.doubleClick(screen.getByText('发布目录对比'));
    expect(openSavedSession).toHaveBeenCalledWith('s-1');
  });
});
