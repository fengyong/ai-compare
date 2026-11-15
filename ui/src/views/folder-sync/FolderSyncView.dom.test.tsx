// @vitest-environment jsdom
/**
 * 工作包 3B 组件级用户故事：文件夹同步（平铺/异名目录 → 同步预览 → 逐项改写
 * → 正确落盘；运行中取消）。
 *
 * 这一层与纯函数测试（syncPlan.test.ts）与真实引擎回放（scripts/e2e-engine.mjs）
 * 互补：这里验证**组件真实渲染 + 真实交互**（下拉改写、按钮点击）后，
 * 送进引擎的执行载荷确实是换向后的真实侧地址。
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../../ipc/engineProcess';
import type { DirDelta, DirExecuteParams, DirRow, PlannedOperation } from '../../protocol/types';

vi.mock('../../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    openFolder: vi.fn(),
    planOperations: vi.fn(),
    executeOperations: vi.fn(),
    cancelEngineRequest: vi.fn(() => true),
    trackSession: vi.fn(),
  };
});

import {
  cancelEngineRequest,
  executeOperations,
  openFolder,
  planOperations,
} from '../../ipc/engineProcess';
import { FolderSyncView } from './FolderSyncView';

const mockOpenFolder = vi.mocked(openFolder);
const mockPlanOperations = vi.mocked(planOperations);
const mockExecuteOperations = vi.mocked(executeOperations);
const mockCancelEngineRequest = vi.mocked(cancelEngineRequest);
import { getDirCache, releaseDirCache } from '../folder-compare/dirRowCache';
import type { Tab } from '../../shell/store';

const SID = 'sync-1';

/** 异名手动对齐后的配对行：左侧 a.txt，右侧 b.txt（R3-03） */
function pairedRow(): DirRow {
  return {
    id: 'a.txt',
    name: 'a.txt',
    kind: 'file',
    left: { size: 3, mtimeMs: 1 },
    right: { size: 3, mtimeMs: 2 },
    status: 'contentDiff',
    hasChildren: false,
    hidden: false,
    suppressed: false,
    leftPath: 'a.txt',
    rightPath: 'b.txt',
  };
}

function seedRows(rows: DirRow[]): void {
  releaseDirCache(SID);
  const delta: DirDelta = {
    sessionId: SID,
    rev: 1,
    upserts: rows,
    removals: [],
    stats: {
      total: rows.length,
      same: 0,
      quickDiff: 0,
      contentDiff: rows.length,
      leftOnly: 0,
      rightOnly: 0,
      dirSame: 0,
      dirDiff: 0,
      similar: 0,
    },
    progress: undefined,
    batch: { index: 0, hasMore: false },
    scanErrors: [],
  };
  getDirCache(SID).applyDelta(delta);
}

function syncTab(): Tab {
  return {
    id: 't-sync',
    kind: 'sync',
    title: 'L ⇄ R（文件夹同步）',
    sync: {
      sessionId: SID,
      left: '/L',
      right: '/R',
    },
  } as Tab;
}

/** 计划：异名配对行的复制操作（源左 a.txt，目标右 b.txt） */
function pairedPlan(): PlannedOperation[] {
  return [
    {
      op: 'copyLeftToRight',
      path: 'a.txt',
      reason: 'preset:MirrorLeftToRight',
      sourcePath: 'a.txt',
      targetPath: 'b.txt',
      displayPath: 'b.txt',
    },
  ];
}

beforeEach(() => {
  vi.clearAllMocks();
  mockOpenFolder.mockResolvedValue({
    sessionId: SID,
    rowCount: 1,
    rev: 1,
    scanState: 'completed',
  });
  mockPlanOperations.mockResolvedValue({
    sessionId: SID,
    plan: pairedPlan(),
    total: 1,
  });
  mockExecuteOperations.mockResolvedValue({
    sessionId: SID,
    total: 1,
    succeeded: [{ op: { kind: 'copy', path: 'a.txt', side: 'left' }, status: 'succeeded' }],
    failed: [],
    skipped: [],
    dryRun: false,
    cancelled: false,
  });
});

afterEach(() => {
  cleanup();
  releaseDirCache(SID);
});

describe('文件夹同步组件（3B：异名目录 → 预览 → 改写 → 落盘）', () => {
  it('异名配对行：预览显示右侧真实地址，执行载荷按左→右真实路径', async () => {
    seedRows([pairedRow()]);
    render(<FolderSyncView tab={syncTab()} />);

    // setup 阶段：等待会话打开完成
    await waitFor(() => expect(screen.getByRole('button', { name: /生成计划|计划/ })).toBeTruthy());

    // 进入计划阶段
    fireEvent.click(screen.getByRole('button', { name: /生成计划|计划/ }));
    await waitFor(() => expect(screen.getByText(/计划/)).toBeTruthy());

    // 预览显示的是**右侧真实地址**（b.txt），不是行 id
    expect(screen.getByText('b.txt')).toBeTruthy();

    // 执行：载荷必须是左→右的真实路径
    fireEvent.click(screen.getByRole('button', { name: /立即同步|执行/ }));
    await waitFor(() => expect(mockExecuteOperations).toHaveBeenCalled());
    const params = mockExecuteOperations.mock.calls[0][0] as DirExecuteParams;
    expect(params.ops).toEqual([
      { kind: 'copy', path: 'a.txt', side: 'left', targetSide: 'right', targetPath: 'b.txt' },
    ]);
  });

  it('逐项改写为「复制 右→左」：执行载荷的源/目标必须换到另一侧真实路径', async () => {
    seedRows([pairedRow()]);
    render(<FolderSyncView tab={syncTab()} />);
    await waitFor(() => expect(screen.getByRole('button', { name: /生成计划|计划/ })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /生成计划|计划/ }));
    await waitFor(() => expect(screen.getByText(/计划/)).toBeTruthy());

    // 改写下拉：复制 右→左
    const selects = screen.getAllByRole('combobox') as HTMLSelectElement[];
    const override = selects[selects.length - 1];
    fireEvent.change(override, { target: { value: 'copyRightToLeft' } });

    fireEvent.click(screen.getByRole('button', { name: /立即同步|执行/ }));
    await waitFor(() => expect(mockExecuteOperations).toHaveBeenCalled());
    const params = mockExecuteOperations.mock.calls[0][0] as DirExecuteParams;
    expect(params.ops).toEqual([
      { kind: 'copy', path: 'b.txt', side: 'right', targetSide: 'left', targetPath: 'a.txt' },
    ]);
  });

  it('用户改写为「保持不变」：不产生任何执行载荷', async () => {
    seedRows([pairedRow()]);
    render(<FolderSyncView tab={syncTab()} />);
    await waitFor(() => expect(screen.getByRole('button', { name: /生成计划|计划/ })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /生成计划|计划/ }));
    await waitFor(() => expect(screen.getByText(/计划/)).toBeTruthy());

    const selects = screen.getAllByRole('combobox') as HTMLSelectElement[];
    const override = selects[selects.length - 1];
    fireEvent.change(override, { target: { value: 'keep' } });

    // keep 后待执行数为 0：执行按钮被禁用，不会发出任何执行请求
    const runBtn = screen.getByRole('button', { name: /立即同步/ }) as HTMLButtonElement;
    await waitFor(() => expect(runBtn.disabled).toBe(true));
    expect(mockExecuteOperations).not.toHaveBeenCalled();
  });

  it('同步运行中取消：后续批次不再执行', async () => {
    // 造 25 个操作（> 一批 20），第一批执行期间触发取消
    const many: PlannedOperation[] = Array.from({ length: 25 }, (_, i) => ({
      op: 'copyLeftToRight' as const,
      path: `f${i}.txt`,
      reason: 'test',
      sourcePath: `f${i}.txt`,
      targetPath: `f${i}.txt`,
      displayPath: `f${i}.txt`,
    }));
    mockPlanOperations.mockResolvedValue({ sessionId: SID, plan: many, total: many.length });
    seedRows([pairedRow()]);

    let resolveFirst: (() => void) | undefined;
    mockExecuteOperations.mockImplementation(
      async () =>
        new Promise((resolve) => {
          resolveFirst = () =>
            resolve({
              sessionId: SID,
              total: 20,
              succeeded: [],
              failed: [],
              skipped: [],
              dryRun: false,
              cancelled: false,
            });
        }),
    );

    render(<FolderSyncView tab={syncTab()} />);
    await waitFor(() => expect(screen.getByRole('button', { name: /生成计划|计划/ })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /生成计划|计划/ }));
    await waitFor(() => expect(screen.getByText(/计划/)).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: /立即同步|执行/ }));

    // 第一批挂起时点取消
    await waitFor(() => expect(mockExecuteOperations).toHaveBeenCalledTimes(1));
    const cancelBtn = screen.getByRole('button', { name: /取消/ });
    fireEvent.click(cancelBtn);
    expect(mockCancelEngineRequest).toHaveBeenCalled();

    resolveFirst?.();
    await waitFor(() => expect(screen.getByText(/取消|已取消/)).toBeTruthy());
    // 取消后不得再发第二批
    expect(mockExecuteOperations).toHaveBeenCalledTimes(1);
  });
});
