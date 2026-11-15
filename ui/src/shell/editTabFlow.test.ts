import { beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../ipc/engineProcess';
import type { EngineCapabilities, TextSaveParams } from '../protocol/types';

vi.mock('../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    openText: vi.fn(),
    saveText: vi.fn(),
    editText: vi.fn(),
    closeSession: vi.fn(),
    trackSession: vi.fn(),
    untrackSession: vi.fn(),
    getEngineCapabilities: vi.fn(),
    newSessionId: vi.fn(() => 'text-fixed-1'),
  };
});

import {
  closeSession,
  editText,
  getEngineCapabilities,
  newSessionId,
  openText,
  saveText,
  trackSession,
  untrackSession,
} from '../ipc/engineProcess';
import { resetShellForTest, useShell } from './store';
import { sessionKindOfTab, specsOfTab } from './sessionSpecs';
import { tabSessionId } from './sliceWindowsTabs';
import { closeGuardKind } from './closeGuard';
import { newEditDoc, putEditDoc, getEditDoc } from '../views/text-edit/editModel';
import { sessionService } from '../session/service';
import type { Tab } from './shellState';

/**
 * D2 壳层接线：单文件编辑标签的打开/参数、会话规格、关闭保护、崩溃重绑。
 *
 * 这些是"只加了个页面"与"完整闭环"的分界：保存语义要能被关闭守卫复用，
 * 会话要能落盘重开，重连后句柄要能重绑。
 */

const SID = 'text-fixed-1';

const mockOpenText = vi.mocked(openText);
const mockSaveText = vi.mocked(saveText);
const mockEditText = vi.mocked(editText);
const mockClose = vi.mocked(closeSession);
const mockTrack = vi.mocked(trackSession);
const mockUntrack = vi.mocked(untrackSession);
const mockCaps = vi.mocked(getEngineCapabilities);
const mockNewSid = vi.mocked(newSessionId);

function caps(over: Partial<EngineCapabilities> = {}): EngineCapabilities {
  return {
    dirCompare: true,
    textCompare: true,
    inlineDiff: true,
    cancel: true,
    deltaBatching: true,
    maxDeltaBatchRows: 512,
    textEdit: true,
    textSave: true,
    textOpenOne: true,
    ...over,
  } as EngineCapabilities;
}

function openResult() {
  return {
    sessionId: SID,
    leftLineCount: 1,
    rightLineCount: 0,
    sectionCount: 0,
    summary: {
      sectionCount: 0,
      equalSections: 0,
      insertSections: 0,
      deleteSections: 0,
      changeSections: 0,
      equalLines: 0,
      insertedLines: 0,
      deletedLines: 0,
    },
    leftEncoding: 'UTF-8',
    rightEncoding: '',
  } as never;
}

function activeTab(): Tab {
  const st = useShell.getState();
  const win = st.windows.find((w) => w.id === st.activeWindowId)!;
  return win.tabs.find((t) => t.id === win.activeTabId)!;
}

beforeEach(() => {
  resetShellForTest();
  vi.clearAllMocks();
  mockCaps.mockReturnValue(caps());
  mockNewSid.mockReturnValue(SID);
  mockOpenText.mockResolvedValue(openResult());
  mockEditText.mockResolvedValue({
    sessionId: SID,
    rev: 1,
    leftLineCount: 1,
    rightLineCount: 0,
    sectionCount: 0,
    summary: {},
    sections: [],
  } as never);
  mockSaveText.mockResolvedValue({
    sessionId: SID,
    saved: [{ side: 'left', path: '/tmp/a.txt', bytes: 3, lineCount: 1 }],
    rev: 1,
    leftModified: false,
    rightModified: false,
  } as never);
  mockClose.mockResolvedValue(null as never);
});

describe('D2 openEditTab 打开单文件会话', () => {
  it('新引擎：text/open 省略 right，标签带编码/行数，会话登记用于崩溃恢复', async () => {
    await useShell.getState().openEditTab('/tmp/a.txt');

    const params = mockOpenText.mock.calls[0][0] as Record<string, unknown>;
    expect(params.left).toBe('/tmp/a.txt');
    expect('right' in params).toBe(false);
    expect(mockTrack).toHaveBeenCalledWith(SID, 'text', { left: '/tmp/a.txt' });

    const tab = activeTab();
    expect(tab.kind).toBe('edit');
    expect(tab.title).toBe('a.txt（文本编辑）');
    expect(tab.edit).toEqual({
      sessionId: SID,
      path: '/tmp/a.txt',
      name: 'a.txt',
      encoding: 'UTF-8',
      lineCount: 1,
    });
  });

  it('旧引擎（无 textOpenOne）：right 退回同路径，行为等价', async () => {
    mockCaps.mockReturnValue(caps({ textOpenOne: undefined }));
    await useShell.getState().openEditTab('/tmp/a.txt');
    expect(mockOpenText.mock.calls[0][0]).toMatchObject({
      left: '/tmp/a.txt',
      right: '/tmp/a.txt',
    });
    expect(mockTrack).toHaveBeenCalledWith(SID, 'text', {
      left: '/tmp/a.txt',
      right: '/tmp/a.txt',
    });
  });

  it('打开失败：提示原因，不创建标签', async () => {
    mockOpenText.mockRejectedValue(new Error('路径不存在'));
    await useShell.getState().openEditTab('/tmp/missing.txt');
    expect(useShell.getState().notice).toContain('打开文件失败');
    expect(useShell.getState().windows[0].tabs.some((t) => t.kind === 'edit')).toBe(false);
  });
});

describe('D2 编辑标签的会话规格与重开', () => {
  it('specsOfTab 落 left + 空 right；kind 映射为 text-edit', async () => {
    await useShell.getState().openEditTab('/tmp/a.txt');
    const tab = activeTab();
    expect(sessionKindOfTab(tab.kind)).toBe('text-edit');
    expect(specsOfTab(tab)).toEqual({ left: '/tmp/a.txt', right: '' });
  });

  it('从会话树重开：按 left 重新打开单文件会话', async () => {
    const node = await sessionService.saveSession({
      kind: 'text-edit',
      name: '编辑 a.txt',
      specs: { left: '/tmp/a.txt', right: '' },
    });
    await useShell.getState().openSavedSession(node.id);
    expect(mockOpenText).toHaveBeenCalled();
    expect(mockOpenText.mock.calls[0][0]).toMatchObject({ left: '/tmp/a.txt' });
    const tab = activeTab();
    expect(tab.kind).toBe('edit');
    expect(tab.sessionLink?.sessionId).toBe(node.id);
  });
});

describe('D2 关闭保护与保存分流', () => {
  async function openAndDirty(): Promise<Tab> {
    await useShell.getState().openEditTab('/tmp/a.txt');
    const tab = activeTab();
    putEditDoc(
      newEditDoc({
        sessionId: SID,
        path: '/tmp/a.txt',
        name: 'a.txt',
        encoding: 'UTF-8',
        lines: ['edited'],
      }),
    );
    // 模拟真实编辑：打开时引擎是 original，用户在编辑器里改成 edited
    const doc = getEditDoc(SID)!;
    doc.engineSnapshot = ['original'];
    doc.savedLines = ['original'];
    useShell.getState().markContentDirty(tab.id, { left: true, right: false });
    return activeTab();
  }

  it('未落盘内容触发 content 守卫；取消后标签保留、内容不丢', async () => {
    const tab = await openAndDirty();
    expect(closeGuardKind(tab)).toBe('content');

    await useShell.getState().closeTab(tab.id);
    expect(useShell.getState().closeGuard).toEqual({ tabId: tab.id, kind: 'content' });
    expect(useShell.getState().windows[0].tabs.some((t) => t.id === tab.id)).toBe(true);

    await useShell.getState().resolveCloseGuard('cancel');
    expect(useShell.getState().windows[0].tabs.some((t) => t.id === tab.id)).toBe(true);
    expect(getEditDoc(SID)!.lines).toEqual(['edited']);
    expect(mockSaveText).not.toHaveBeenCalled();
  });

  it('选择保存：先收敛引擎内容再落盘，清脏后关闭并释放会话', async () => {
    const tab = await openAndDirty();
    await useShell.getState().closeTab(tab.id);
    await useShell.getState().resolveCloseGuard('save');

    expect(mockEditText).toHaveBeenCalled(); // 收敛：把 edited 交给引擎
    const saveParams = mockSaveText.mock.calls[0][0] as TextSaveParams;
    expect(saveParams).toMatchObject({ sessionId: SID, side: 'left' });
    expect(useShell.getState().windows[0].tabs.some((t) => t.id === tab.id)).toBe(false);
    expect(mockUntrack).toHaveBeenCalledWith(SID);
    expect(mockClose).toHaveBeenCalledWith({ sessionId: SID });
  });

  it('Ctrl+S（session.save）在编辑标签上保存文件而不是提示命名会话', async () => {
    const tab = await openAndDirty();
    await useShell.getState().saveSessionForTab(tab.id);
    expect(mockSaveText).toHaveBeenCalledTimes(1);
    expect(useShell.getState().saveAsPrompt).toBeNull();
    expect(useShell.getState().findTab(tab.id)!.tab.contentDirty?.left).toBe(false);
  });

  it('A0 保存期间又有编辑：关闭被中断，标签保留（新输入不得静默丢失）', async () => {
    const tab = await openAndDirty();
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => {
      release = r;
    });
    mockSaveText.mockImplementation(async () => {
      await gate;
      return {
        sessionId: SID,
        saved: [{ side: 'left', path: '/tmp/a.txt', bytes: 3, lineCount: 1 }],
        rev: 1,
        leftModified: false,
        rightModified: false,
      } as never;
    });

    await useShell.getState().closeTab(tab.id);
    const pending = useShell.getState().resolveCloseGuard('save');
    await vi.waitFor(() => expect(mockSaveText).toHaveBeenCalled());

    // 保存应答回来之前，用户又敲了内容
    getEditDoc(SID)!.lines = ['edited', 'again'];
    release();
    await pending;

    expect(useShell.getState().windows[0].tabs.some((t) => t.id === tab.id)).toBe(true);
    expect(useShell.getState().findTab(tab.id)!.tab.contentDirty?.left).toBe(true);
    expect(useShell.getState().notice).toContain('保存期间');
  });

  it('保存失败：关闭被中断，标签与内容保留（不静默丢弃）', async () => {
    const tab = await openAndDirty();
    mockSaveText.mockRejectedValue(new Error('磁盘只读'));
    await useShell.getState().closeTab(tab.id);
    await useShell.getState().resolveCloseGuard('save');
    expect(useShell.getState().windows[0].tabs.some((t) => t.id === tab.id)).toBe(true);
    expect(useShell.getState().notice).toContain('磁盘只读');
  });
});

describe('D2 崩溃恢复：会话句柄重绑', () => {
  it('重连后 edit 标签的句柄被替换，旧句柄不再被使用', async () => {
    await useShell.getState().openEditTab('/tmp/a.txt');
    const tab = activeTab();
    expect(tabSessionId(tab)).toBe(SID);
    useShell.getState().rebindSession(SID, 'text-new-9');
    expect(tabSessionId(activeTab())).toBe('text-new-9');
  });
});
