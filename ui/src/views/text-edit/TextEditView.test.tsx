// @vitest-environment jsdom
/**
 * 独立文本编辑视图组件测试（D2）。
 *
 * 覆盖"打开 → 编辑 → 脏标记 → 保存/失败 → 只读降级"的真实组件链路：
 * 引擎被 mock（协议形状同真实引擎），编辑器是**真实 CM6 实例**（经
 * `EditorView.findFromDOM` 派发编辑事务，等价用户输入）。
 * 真实引擎与真实字节由 scripts/e2e-engine.mjs 覆盖。
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { EditorView } from '@codemirror/view';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../../ipc/engineProcess';
import type { EngineCapabilities, TextEditParams, TextSaveParams } from '../../protocol/types';
import type { Tab } from '../../shell/shellState';
import { RpcError } from '../../protocol/client';

vi.mock('../../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    openText: vi.fn(),
    loadSideLines: vi.fn(),
    editText: vi.fn(),
    saveText: vi.fn(),
    trackSession: vi.fn(),
    getEngineCapabilities: vi.fn(),
  };
});

import {
  editText,
  getEngineCapabilities,
  loadSideLines,
  openText,
  saveText,
  trackSession,
} from '../../ipc/engineProcess';
import { resetShellForTest, useShell } from '../../shell/store';
import { S } from '../../strings';
import { deleteEditDoc, getEditDoc, newEditDoc, putEditDoc } from './editModel';
import { TextEditView } from './TextEditView';

const mockOpenText = vi.mocked(openText);
const mockLoadLines = vi.mocked(loadSideLines);
const mockEditText = vi.mocked(editText);
const mockSaveText = vi.mocked(saveText);
const mockTrack = vi.mocked(trackSession);
const mockCaps = vi.mocked(getEngineCapabilities);

const SID = 'text-9-abc';
const TAB_ID = 'tab-edit-1';

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

function editTab(): Tab {
  return {
    id: TAB_ID,
    kind: 'edit',
    title: 'a.txt（文本编辑）',
    edit: {
      sessionId: SID,
      path: '/tmp/a.txt',
      name: 'a.txt',
      encoding: 'GB18030',
      lineCount: 2,
    },
  };
}

/**
 * 有状态的假 text/edit：按真实语义应用行区间（否则行数恒定的 mock 会让
 * "收敛校验"永远判分叉，测不出真实链路）。
 */
function installFakeEngine(initial: string[]): void {
  const table = [...initial];
  mockEditText.mockImplementation(async (params) => {
    for (const op of (params as TextEditParams).edits) {
      const end = op.end ?? table.length;
      const parts = op.text === '' ? [] : op.text.split('\n');
      if (parts.length > 1 && parts[parts.length - 1] === '') parts.pop();
      table.splice(op.start, end - op.start, ...parts);
    }
    return {
      sessionId: SID,
      rev: 1,
      leftLineCount: table.length,
      rightLineCount: 0,
      sectionCount: 0,
      summary: {},
      sections: [],
    } as never;
  });
}

function seedTab(tab: Tab): void {
  useShell.setState((st) => ({
    windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, tab], activeTabId: tab.id })),
  }));
}

/** text/open 应答（A3：`leftTrailingEol` 缺省 = 旧引擎不回报） */
function openResult(over: Record<string, unknown> = {}): never {
  return {
    sessionId: SID,
    leftLineCount: 2,
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
    leftEncoding: 'GB18030',
    rightEncoding: '',
    ...over,
  } as never;
}

/** 取得组件内部真实 CM 实例（等价用户输入入口） */
function cmView(container: HTMLElement): EditorView {
  const content = container.querySelector('.cm-content');
  expect(content).toBeTruthy();
  const view = EditorView.findFromDOM(content as HTMLElement);
  expect(view).toBeTruthy();
  return view as EditorView;
}

beforeEach(() => {
  resetShellForTest();
  // 会话级缓存与通知都是模块级状态：逐个用例清空，避免相互污染
  deleteEditDoc(SID);
  useShell.setState({ notice: null });
  vi.clearAllMocks();
  mockCaps.mockReturnValue(caps());
  mockOpenText.mockResolvedValue(openResult());
  mockLoadLines.mockResolvedValue(['第一行', '第二行']);
  installFakeEngine(['第一行', '第二行']);
  mockSaveText.mockResolvedValue({
    sessionId: SID,
    saved: [
      { side: 'left', path: '/tmp/a.txt', bytes: 24, lineCount: 2, backupPath: '/tmp/a.txt.bak' },
    ],
    rev: 1,
    leftModified: false,
    rightModified: false,
  } as never);
});

afterEach(() => {
  cleanup();
});

describe('TextEditView 打开', () => {
  it('单文件会话：text/open 省略 right；显示文件名、编码与行数', async () => {
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);

    await waitFor(() => expect(screen.getByText(/第一行/)).toBeTruthy());
    const params = mockOpenText.mock.calls[0][0];
    expect(params.left).toBe('/tmp/a.txt');
    expect('right' in params).toBe(false);
    expect(mockLoadLines).toHaveBeenCalledWith(SID, 'left', 2, '/tmp/a.txt');
    expect(mockTrack).toHaveBeenCalledWith(SID, 'text', { left: '/tmp/a.txt' });
    // 编码在工具栏徽标与状态栏各显示一处
    expect(screen.getAllByText(/GB18030/).length).toBeGreaterThan(0);
    expect(screen.getByText(/2 行/)).toBeTruthy();
    expect(cmView(container).state.doc.toString()).toBe('第一行\n第二行');
  });

  it('旧引擎（无 textOpenOne）：退回同路径双开，行为等价', async () => {
    mockCaps.mockReturnValue(caps({ textOpenOne: undefined }));
    seedTab(editTab());
    render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(mockOpenText).toHaveBeenCalled());
    expect(mockOpenText.mock.calls[0][0]).toMatchObject({
      left: '/tmp/a.txt',
      right: '/tmp/a.txt',
    });
  });
});

describe('TextEditView 编辑与脏标记', () => {
  it('编辑后标记内容脏，并把左侧增量交给 text/edit', async () => {
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(cmView(container).state.doc.length).toBeGreaterThan(0));

    cmView(container).dispatch({ changes: { from: 0, insert: 'X' } });

    await waitFor(() =>
      expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty).toEqual({
        left: true,
        right: false,
      }),
    );
    await waitFor(() => expect(mockEditText).toHaveBeenCalled());
    const params = mockEditText.mock.calls[0][0] as TextEditParams;
    expect(params.sessionId).toBe(SID);
    expect(params.edits).toHaveLength(1);
    expect(params.edits[0].side).toBe('left');
    expect(params.edits[0].text).toBe('X第一行');
    // 模型同步更新（引擎快照 = UI 真值）
    expect(getEditDoc(SID)!.engineSnapshot).toEqual(['X第一行', '第二行']);
  });
});

describe('TextEditView 保存', () => {
  it('保存：side=left + 备份开关；成功后清脏并提示备份路径', async () => {
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(cmView(container).state.doc.length).toBeGreaterThan(0));
    cmView(container).dispatch({ changes: { from: 0, insert: 'X' } });
    await waitFor(() => expect(screen.getByTitle(/有未保存编辑/)).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: '已保存' }));

    await waitFor(() => expect(mockSaveText).toHaveBeenCalled());
    const params = mockSaveText.mock.calls[0][0] as TextSaveParams;
    expect(params).toEqual({ sessionId: SID, side: 'left', backup: true, leftPath: undefined });
    await waitFor(() =>
      expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty).toEqual({
        left: false,
        right: false,
      }),
    );
    expect(useShell.getState().notice).toContain('.bak');
  });

  it('保存失败（编码无法表示）：不清脏、提示原因、不谎报成功', async () => {
    seedTab(editTab());
    mockSaveText.mockRejectedValue(new RpcError(413, 'encoding', { message: 'encoding' } as never));
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(cmView(container).state.doc.length).toBeGreaterThan(0));
    cmView(container).dispatch({ changes: { from: 0, insert: 'X' } });
    await waitFor(() => expect(screen.getByTitle(/有未保存编辑/)).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: '已保存' }));

    await waitFor(() => expect(useShell.getState().notice).toContain('编码无法表示'));
    expect(useShell.getState().findTab(TAB_ID)!.tab.contentDirty?.left).toBe(true);
    expect(screen.getByTitle(/有未保存编辑/)).toBeTruthy();
  });
});

describe('TextEditView 崩溃重放', () => {
  it('新句柄重开同一文件：认领旧会话的未落盘编辑并重新收敛到引擎', async () => {
    // 旧会话（已被重放替换）里的未保存编辑
    const stale = newEditDoc({
      sessionId: 'text-old',
      path: '/tmp/a.txt',
      name: 'a.txt',
      encoding: 'GB18030',
      lines: ['我的修改'],
    });
    stale.savedLines = ['第一行'];
    stale.engineSnapshot = ['第一行'];
    putEditDoc(stale);

    const tab = editTab();
    useShell.setState((st) => ({
      windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, tab], activeTabId: tab.id })),
    }));
    const { container } = render(<TextEditView tab={tab} />);

    // 引擎按新句柄重开（内容回到文件态：第一行/第二行）
    await waitFor(() => expect(mockOpenText).toHaveBeenCalled());
    // UI 内容仍是用户那份修改，且被重新交给引擎
    await waitFor(() => expect(mockEditText).toHaveBeenCalled());
    const doc = getEditDoc(SID)!;
    expect(doc.lines).toEqual(['我的修改']);
    expect(doc.savedLines).toEqual(['第一行']);
    expect(doc.engineSnapshot).toEqual(['我的修改']);
    expect(getEditDoc('text-old')).toBeUndefined();
    await waitFor(() => expect(cmView(container).state.doc.toString()).toBe('我的修改'));
  });
});

describe('TextEditView A3（末行行尾符）', () => {
  it('引擎回报末尾有换行：文档按它构建，状态栏给出标记', async () => {
    mockOpenText.mockResolvedValue(openResult({ leftTrailingEol: true }));
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);

    await waitFor(() => expect(screen.getByText(/第一行/)).toBeTruthy());
    // "a\n" 与 "a" 都是 2 行——差别只在末行之后有没有换行
    expect(cmView(container).state.doc.toString()).toBe('第一行\n第二行\n');
    expect(screen.getByText(S.edit.eofOn)).toBeTruthy();
  });

  it('旧引擎不回报该字段：退回旧语义且不显示标记', async () => {
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);

    await waitFor(() => expect(screen.getByText(/第一行/)).toBeTruthy());
    expect(cmView(container).state.doc.toString()).toBe('第一行\n第二行');
    expect(screen.queryByText(S.edit.eofOn)).toBeNull();
    expect(screen.queryByText(S.edit.eofOff)).toBeNull();
  });

  it('删掉末尾换行：只发 EOF 标记，行内容不动', async () => {
    mockOpenText.mockResolvedValue(openResult({ leftTrailingEol: true }));
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(cmView(container).state.doc.length).toBeGreaterThan(0));

    const view = cmView(container);
    const len = view.state.doc.length;
    view.dispatch({ changes: { from: len - 1, to: len } }); // 删掉文件末尾那个 '\n'

    await waitFor(() => expect(mockEditText).toHaveBeenCalled());
    const params = mockEditText.mock.calls.at(-1)![0];
    expect(params.edits).toEqual([]);
    expect(params.leftTrailingEol).toBe(false);
    expect(screen.getByText(S.edit.eofOff)).toBeTruthy();
  });

  it('全部替换保留末尾换行（WT-01）：尾换行文件替换后 EOF 标记不丢', async () => {
    mockOpenText.mockResolvedValue(openResult({ leftTrailingEol: true }));
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(cmView(container).state.doc.toString()).toBe('第一行\n第二行\n'));

    // 打开查找替换条，输入 needle 与替换文本
    fireEvent.click(screen.getByRole('button', { name: S.edit.find }));
    fireEvent.change(screen.getByPlaceholderText(S.edit.findPlaceholder), {
      target: { value: '第一行' },
    });
    fireEvent.change(screen.getByPlaceholderText(S.edit.replacePlaceholder), {
      target: { value: '替换行' },
    });
    fireEvent.click(screen.getByRole('button', { name: S.edit.replaceAll }));

    // 替换命中且整篇重写后，末尾换行仍在（不得静默丢字节）
    await waitFor(() =>
      expect(cmView(container).state.doc.toString()).toBe('替换行\n第二行\n'),
    );
    expect(getEditDoc(SID)!.trailingEol).toBe(true);
    expect(screen.getByText(S.edit.eofOn)).toBeTruthy();
  });

  it('全部替换不给无尾换行文件补换行（WT-01 反向）：EOF 标记保持 false', async () => {
    mockOpenText.mockResolvedValue(openResult({ leftTrailingEol: false }));
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(cmView(container).state.doc.toString()).toBe('第一行\n第二行'));

    fireEvent.click(screen.getByRole('button', { name: S.edit.find }));
    fireEvent.change(screen.getByPlaceholderText(S.edit.findPlaceholder), {
      target: { value: '第一行' },
    });
    fireEvent.change(screen.getByPlaceholderText(S.edit.replacePlaceholder), {
      target: { value: '替换行' },
    });
    fireEvent.click(screen.getByRole('button', { name: S.edit.replaceAll }));

    await waitFor(() =>
      expect(cmView(container).state.doc.toString()).toBe('替换行\n第二行'),
    );
    expect(getEditDoc(SID)!.trailingEol).toBe(false);
    expect(screen.getByText(S.edit.eofOff)).toBeTruthy();
  });
});

describe('TextEditView 能力降级', () => {
  it('引擎无 text/edit：只读打开、保存按钮禁用（不得假报可保存）', async () => {
    mockCaps.mockReturnValue(caps({ textEdit: undefined }));
    seedTab(editTab());
    const { container } = render(<TextEditView tab={editTab()} />);
    await waitFor(() => expect(screen.getByText(/只读/)).toBeTruthy());
    expect(cmView(container).contentDOM.getAttribute('contenteditable')).toBe('false');
    const saveBtn = screen.getByRole('button', { name: '已保存' }) as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
    expect(useShell.getState().notice).toContain('不支持编辑同步');
  });
});
