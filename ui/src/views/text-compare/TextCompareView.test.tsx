// @vitest-environment jsdom
/**
 * 文本比较视图崩溃认领回归（UI-01）。
 *
 * 引擎崩溃重放给会话换新句柄，视图按新句柄重开同一文件对（引擎内容回到
 * 文件态）——旧（已死）会话缓存里的未落盘编辑必须按路径认领回来：
 * UI 内容/脏标记保留，且经 text/edit 整侧重放重新收敛进新会话。
 * 引擎被 mock（协议形状同真实引擎），窗格是真实 CM6 实例。
 */
import { cleanup, render, waitFor } from '@testing-library/react';
import { EditorView } from '@codemirror/view';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../../ipc/engineProcess';
import type { EngineCapabilities, Section, TextEditParams } from '../../protocol/types';
import type { Tab } from '../../shell/shellState';

vi.mock('../../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    openText: vi.fn(),
    getTextSections: vi.fn(),
    loadSideLines: vi.fn(),
    editText: vi.fn(),
    saveText: vi.fn(),
    requestInlineDiff: vi.fn(),
    getEngineCapabilities: vi.fn(),
    closeSession: vi.fn(),
    filterDir: vi.fn(),
  };
});

import {
  editText,
  getEngineCapabilities,
  getTextSections,
  loadSideLines,
  openText,
  requestInlineDiff,
  trackSession,
  untrackSession,
} from '../../ipc/engineProcess';
import { resetShellForTest, useShell } from '../../shell/store';
import {
  deleteTextSession,
  getTextSession,
  newTextSessionModel,
  putTextSession,
} from './textModel';
import { TextCompareView } from './TextCompareView';

const mockOpenText = vi.mocked(openText);
const mockSections = vi.mocked(getTextSections);
const mockLoadLines = vi.mocked(loadSideLines);
const mockEditText = vi.mocked(editText);
const mockInline = vi.mocked(requestInlineDiff);
const mockCaps = vi.mocked(getEngineCapabilities);

// jsdom 无 ResizeObserver（OverviewBar 用）；绘制本身对 0 尺寸/null ctx 已判空
vi.stubGlobal(
  'ResizeObserver',
  class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  },
);

const OLD_SID = 'text-old';
const NEW_SID = 'text-new';
const TAB_ID = 'tab-cmp-1';
const L_PATH = '/tmp/a.txt';
const R_PATH = '/tmp/b.txt';
const FILE_LEFT = ['左一', '左二'];
const FILE_RIGHT = ['右一', '右二'];

const ZERO_SUMMARY = {
  sectionCount: 0,
  equalSections: 0,
  insertSections: 0,
  deleteSections: 0,
  changeSections: 0,
  equalLines: 0,
  insertedLines: 0,
  deletedLines: 0,
};

/** 整文件单段 sections（对齐文档由 sections 展开，空 sections 会渲染空文档） */
function sectionsFor(left: string[], right: string[]): Section[] {
  const equal =
    left.length === right.length && left.every((l, i) => l === right[i]);
  return [
    {
      id: 1,
      kind: equal ? 'equal' : 'change',
      left: { start: 0, end: left.length },
      right: { start: 0, end: right.length },
      important: 'important',
      inline: 'notComputed',
    },
  ];
}

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
    ...over,
  } as EngineCapabilities;
}

function compareTab(): Tab {
  return {
    id: TAB_ID,
    kind: 'text',
    title: 'a.txt vs b.txt',
    text: {
      sessionId: NEW_SID,
      left: { path: L_PATH, name: 'a.txt' },
      right: { path: R_PATH, name: 'b.txt' },
    },
  };
}

/** 有状态的假 text/edit：按真实语义应用两侧行区间（收敛校验需要真行数） */
function installFakeEngine(): { left: string[]; right: string[] } {
  const tables = { left: [...FILE_LEFT], right: [...FILE_RIGHT] };
  mockEditText.mockImplementation(async (params) => {
    for (const op of (params as TextEditParams).edits) {
      const table = tables[op.side];
      const end = op.end ?? table.length;
      const parts = op.text === '' ? [] : op.text.split('\n');
      if (parts.length > 1 && parts[parts.length - 1] === '') parts.pop();
      table.splice(op.start, end - op.start, ...parts);
    }
    return {
      sessionId: NEW_SID,
      rev: 1,
      leftLineCount: tables.left.length,
      rightLineCount: tables.right.length,
      sectionCount: 1,
      summary: ZERO_SUMMARY,
      sections: sectionsFor(tables.left, tables.right),
    } as never;
  });
  return tables;
}

/** 旧会话模型（崩溃前留在 UI 缓存里的那份） */
function seedStale(over: {
  leftLines?: string[];
  rightLines?: string[];
  modifiedSides?: { left: boolean; right: boolean };
}): void {
  const model = newTextSessionModel({
    sessionId: OLD_SID,
    leftPath: L_PATH,
    rightPath: R_PATH,
    leftName: 'a.txt',
    rightName: 'b.txt',
    leftLines: over.leftLines ?? [...FILE_LEFT],
    rightLines: over.rightLines ?? [...FILE_RIGHT],
    sections: [],
    summary: { ...ZERO_SUMMARY },
  });
  model.modifiedSides = over.modifiedSides ?? { left: false, right: false };
  putTextSession(model);
}

function seedTab(tab: Tab): void {
  useShell.setState((st) => ({
    windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, tab], activeTabId: tab.id })),
  }));
}

function cmDocs(container: HTMLElement): EditorView[] {
  const contents = container.querySelectorAll('.cm-content');
  return Array.from(contents).map((el) => {
    const view = EditorView.findFromDOM(el as HTMLElement);
    expect(view).toBeTruthy();
    return view as EditorView;
  });
}

beforeEach(() => {
  resetShellForTest();
  deleteTextSession(OLD_SID);
  deleteTextSession(NEW_SID);
  untrackSession(OLD_SID);
  untrackSession(NEW_SID);
  useShell.setState({ notice: null });
  vi.clearAllMocks();
  mockCaps.mockReturnValue(caps());
  mockOpenText.mockResolvedValue({
    sessionId: NEW_SID,
    leftLineCount: 2,
    rightLineCount: 2,
    sectionCount: 0,
    summary: { ...ZERO_SUMMARY },
    leftEncoding: 'UTF-8',
    rightEncoding: 'UTF-8',
  } as never);
  mockSections.mockResolvedValue({
    sections: sectionsFor(FILE_LEFT, FILE_RIGHT),
    hasMore: false,
  } as never);
  mockInline.mockResolvedValue(null);
  mockLoadLines.mockImplementation(async (_sid, side) =>
    side === 'left' ? [...FILE_LEFT] : [...FILE_RIGHT],
  );
  installFakeEngine();
});

afterEach(() => {
  cleanup();
});

describe('TextCompareView 崩溃认领（UI-01）', () => {
  it('重放换句柄后认领旧会话未落盘编辑：内容/脏标记保留并整侧重放收敛', async () => {
    seedStale({
      leftLines: ['左一', '左二改'],
      modifiedSides: { left: true, right: false },
    });
    const tab = compareTab();
    seedTab(tab);
    const { container } = render(<TextCompareView tab={tab} />);

    await waitFor(() => expect(cmDocs(container).length).toBe(2));
    // UI 真值 = 认领回来的编辑，不是引擎重开的文件态
    await waitFor(() =>
      expect(cmDocs(container)[0].state.doc.toString()).toContain('左二改'),
    );
    expect(cmDocs(container)[1].state.doc.toString()).toContain('右一');
    // 脏标记登记到标签（关闭守卫可见）
    expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty).toEqual({
      left: true,
      right: false,
    });
    // 只重放脏侧（干净侧重放会把引擎误标为已修改）
    await waitFor(() => expect(mockEditText).toHaveBeenCalled());
    const params = mockEditText.mock.calls[0][0] as TextEditParams;
    expect(params.sessionId).toBe(NEW_SID);
    expect(params.edits).toEqual([{ side: 'left', start: 0, text: '左一\n左二改' }]);
    // 旧键模型删除、新键模型持有认领内容
    expect(getTextSession(OLD_SID)).toBeUndefined();
    expect(getTextSession(NEW_SID)!.leftLines).toEqual(['左一', '左二改']);
  });

  it('双侧都脏：两侧各一条整侧重放 op', async () => {
    seedStale({
      leftLines: ['左一改'],
      rightLines: ['右一', '右二', '右三新增'],
      modifiedSides: { left: true, right: true },
    });
    const tab = compareTab();
    seedTab(tab);
    const { container } = render(<TextCompareView tab={tab} />);

    await waitFor(() => expect(mockEditText).toHaveBeenCalled());
    const params = mockEditText.mock.calls[0][0] as TextEditParams;
    expect(params.edits).toEqual([
      { side: 'left', start: 0, text: '左一改' },
      { side: 'right', start: 0, text: '右一\n右二\n右三新增' },
    ]);
    await waitFor(() =>
      expect(cmDocs(container)[1].state.doc.toString()).toContain('右三新增'),
    );
    expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty).toEqual({
      left: true,
      right: true,
    });
  });

  it('旧会话仍活着（另一标签在编辑同文件对）：不得抢占其编辑', async () => {
    seedStale({
      leftLines: ['别人的未保存编辑'],
      modifiedSides: { left: true, right: false },
    });
    trackSession(OLD_SID, 'text', { left: L_PATH, right: R_PATH });
    const tab = compareTab();
    seedTab(tab);
    const { container } = render(<TextCompareView tab={tab} />);

    await waitFor(() => expect(cmDocs(container).length).toBe(2));
    await waitFor(() =>
      expect(cmDocs(container)[0].state.doc.toString()).toContain('左一'),
    );
    expect(cmDocs(container)[0].state.doc.toString()).not.toContain('别人的未保存编辑');
    expect(getTextSession(OLD_SID)).toBeTruthy();
    expect(mockEditText).not.toHaveBeenCalled();
    expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty).toBeUndefined();
  });

  it('旧模型无未落盘编辑：不认领（引擎文件态即真值）', async () => {
    seedStale({ leftLines: ['与引擎一致'], modifiedSides: { left: false, right: false } });
    const tab = compareTab();
    seedTab(tab);
    const { container } = render(<TextCompareView tab={tab} />);

    await waitFor(() => expect(cmDocs(container).length).toBe(2));
    await waitFor(() =>
      expect(cmDocs(container)[0].state.doc.toString()).toContain('左一'),
    );
    expect(mockEditText).not.toHaveBeenCalled();
    expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty).toBeUndefined();
  });
});

describe('TextCompareView 认领错误路径（UI-02）', () => {
  it('重建未完成（在途/早退/抛出）时认领内容已先落新键缓存——删旧键不留空窗', async () => {
    // 行内差异 RPC 停在在途：rebuildAndRender 到不了其内部 putTextSession。
    // 跳会话早退（liveSessionRef 核对）/RPC 拒绝抛出都会把这一"未完成"态变成永久态，
    // 故此处用闸门冻结重建，直接观测"删旧键后、写回前"缓存里是否已有认领内容。
    let release!: (v: null) => void;
    const gate = new Promise<null>((r) => {
      release = r;
    });
    mockInline.mockReturnValue(gate);
    seedStale({
      leftLines: ['左一', '左二改'],
      modifiedSides: { left: true, right: false },
    });
    const tab = compareTab();
    seedTab(tab);
    const { container } = render(<TextCompareView tab={tab} />);

    // 认领块（同步）已跑完、重建已进入在途 RPC
    await waitFor(() => expect(mockInline).toHaveBeenCalled());
    expect(getTextSession(OLD_SID)).toBeUndefined();
    const cached = getTextSession(NEW_SID);
    expect(cached, '重建未完成时认领内容必须已在新键缓存').toBeDefined();
    expect(cached!.leftLines).toEqual(['左一', '左二改']);
    expect(cached!.modifiedSides).toEqual({ left: true, right: false });

    release(null); // 放行在途 RPC，收尾让重建完成
    await waitFor(() => expect(cmDocs(container).length).toBe(2));
  });

  it('行内差异 RPC 拒绝：降级词级兜底，认领内容照常渲染且仍在缓存（不整屏报错）', async () => {
    // requestInlineDiff 的 getEngineClient 在 try 外，可拒绝；无 catch 时 Promise.all
    // 抛出 → rebuildAndRender 抛出 → setError、窗格不渲染（认领内容看不到）。
    mockInline.mockRejectedValue(new Error('engine client gone'));
    seedStale({
      leftLines: ['左一', '左二改'],
      modifiedSides: { left: true, right: false },
    });
    const tab = compareTab();
    seedTab(tab);
    const { container } = render(<TextCompareView tab={tab} />);

    await waitFor(() => expect(cmDocs(container).length).toBe(2));
    await waitFor(() =>
      expect(cmDocs(container)[0].state.doc.toString()).toContain('左二改'),
    );
    expect(getTextSession(OLD_SID)).toBeUndefined();
    expect(getTextSession(NEW_SID)!.leftLines).toEqual(['左一', '左二改']);
  });
});