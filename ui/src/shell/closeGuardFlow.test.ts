import { beforeEach, describe, expect, it, vi } from 'vitest';
import { resetShellForTest, useShell } from './store';
import * as engine from '../ipc/engineProcess';
import {
  getTextSession,
  newTextSessionModel,
  putTextSession,
} from '../views/text-compare/textModel';
import type { TextSaveParams, TextSaveResult } from '../protocol/types';
import type { Tab } from './shellState';

/**
 * R3-05 回归：未落盘的文件编辑必须触发可靠的关闭保护。
 *
 * 此前守卫只看 `sessionLink && dirty`，而文本编辑从不登记 dirty：
 * 未命名标签的编辑可被直接关闭丢弃，批量关闭/退出更是整批静默丢弃。
 */

const SID = 's-text-1';

function textTab(id: string, over: Partial<Tab> = {}): Tab {
  return {
    id,
    kind: 'text',
    title: 'x',
    text: {
      sessionId: `${SID}-${id}`,
      left: { path: '/l.txt', name: 'l.txt' },
      right: { path: '/r.txt', name: 'r.txt' },
    },
    ...over,
  };
}

function addTabs(...tabs: Tab[]): string {
  useShell.setState((st) => ({
    windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, ...tabs] })),
  }));
  return useShell.getState().windows[0].id;
}

function seedModel(tab: Tab, dirty: { left: boolean; right: boolean }): void {
  const model = newTextSessionModel({
    sessionId: tab.text!.sessionId,
    leftPath: tab.text!.left.path,
    rightPath: tab.text!.right.path,
    leftName: tab.text!.left.name,
    rightName: tab.text!.right.name,
    leftLines: ['a'],
    rightLines: ['b'],
    sections: [],
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
  });
  model.modifiedSides = { ...dirty };
  putTextSession(model);
}

function saveResult(sessionId: string, sides: Array<'left' | 'right'>): TextSaveResult {
  return {
    sessionId,
    saved: sides.map((side) => ({
      side,
      path: side === 'left' ? '/l.txt' : '/r.txt',
      bytes: 2,
      lineCount: 1,
      backupPath: `${side === 'left' ? '/l.txt' : '/r.txt'}.bak`,
    })),
    // R3-21：生成类型与 NDJSON 均为 JSON number
    rev: 1,
    leftModified: false,
    rightModified: false,
  };
}

describe('R3-05 关闭守卫：文件编辑链路', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    vi.spyOn(engine, 'closeSession').mockResolvedValue(null as never);
  });

  it('编辑登记到标签 → 未命名文本标签关闭时也必须提示', async () => {
    const tab = textTab('t1');
    addTabs(tab);
    useShell.getState().markContentDirty('t1', { left: true, right: false });

    await useShell.getState().closeTab('t1');

    expect(useShell.getState().closeGuard).toEqual({ tabId: 't1', kind: 'content' });
    expect(useShell.getState().findTab('t1')).toBeDefined();
  });

  it('markContentDirty 值未变化时不产生新状态（避免每次 keystroke 重渲染）', () => {
    addTabs(textTab('t1'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    const before = useShell.getState().windows;
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    expect(useShell.getState().windows).toBe(before);
  });

  it('「保存并关闭」按脏侧落盘后才关闭，并清掉标签脏标记与会话缓存', async () => {
    const tab = textTab('t1');
    addTabs(tab);
    seedModel(tab, { left: true, right: true });
    useShell.getState().markContentDirty('t1', { left: true, right: true });
    const calls: TextSaveParams[] = [];
    vi.spyOn(engine, 'saveText').mockImplementation(async (p: TextSaveParams) => {
      calls.push(p);
      return saveResult(p.sessionId, [p.side as 'left' | 'right']);
    });

    await useShell.getState().closeTab('t1');
    await useShell.getState().resolveCloseGuard('save');

    expect(calls.map((c) => c.side)).toEqual(['left', 'right']);
    expect(calls.every((c) => c.sessionId === tab.text!.sessionId)).toBe(true);
    expect(useShell.getState().findTab('t1')).toBeUndefined();
    expect(useShell.getState().closeGuard).toBeNull();
    expect(getTextSession(tab.text!.sessionId)).toBeUndefined();
  });

  it('落盘失败 → 不关闭标签，编辑保留（不得静默丢弃）', async () => {
    addTabs(textTab('t1'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    vi.spyOn(engine, 'saveText').mockRejectedValue(new Error('磁盘只读'));

    await useShell.getState().closeTab('t1');
    await useShell.getState().resolveCloseGuard('save');

    expect(useShell.getState().findTab('t1')).toBeDefined();
    expect(useShell.getState().findTab('t1')!.tab.contentDirty).toEqual({
      left: true,
      right: false,
    });
    expect(useShell.getState().notice).toContain('磁盘只读');
    expect(useShell.getState().closeGuard).toBeNull();
  });

  it('引擎无 text/save 能力 → 提示未保存且不关闭', async () => {
    addTabs(textTab('t1'));
    useShell.getState().markContentDirty('t1', { left: false, right: true });
    vi.spyOn(engine, 'saveText').mockResolvedValue(null);

    await useShell.getState().closeTab('t1');
    await useShell.getState().resolveCloseGuard('save');

    expect(useShell.getState().findTab('t1')).toBeDefined();
    expect(useShell.getState().notice).toContain('text/save');
  });

  it('「不保存关闭」丢弃编辑并关闭；「取消」保留标签与脏标记', async () => {
    addTabs(textTab('t1'), textTab('t2'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    useShell.getState().markContentDirty('t2', { left: true, right: false });

    await useShell.getState().closeTab('t1');
    await useShell.getState().resolveCloseGuard('cancel');
    expect(useShell.getState().findTab('t1')!.tab.contentDirty).toEqual({
      left: true,
      right: false,
    });

    await useShell.getState().closeTab('t2');
    await useShell.getState().resolveCloseGuard('discard');
    expect(useShell.getState().findTab('t2')).toBeUndefined();
  });

  it('文件编辑 + 命名会话同时脏 → 先保存文件，再提示保存会话（不得跳过）', async () => {
    const tab = textTab('t1', {
      sessionLink: { sessionId: 'ns', name: '命名', kind: 'text-compare', locked: false },
      dirty: true,
    });
    addTabs(tab);
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    vi.spyOn(engine, 'saveText').mockImplementation(async (p: TextSaveParams) =>
      saveResult(p.sessionId, ['left']),
    );
    const saveSession = vi.fn(async () => undefined);
    useShell.setState({ saveSessionForTab: saveSession });

    await useShell.getState().closeTab('t1');
    expect(useShell.getState().closeGuard?.kind).toBe('content');
    await useShell.getState().resolveCloseGuard('save');

    expect(useShell.getState().closeGuard).toEqual({ tabId: 't1', kind: 'session' });
    expect(useShell.getState().findTab('t1')).toBeDefined();
    expect(saveSession).not.toHaveBeenCalled();

    await useShell.getState().resolveCloseGuard('save');
    expect(saveSession).toHaveBeenCalledWith('t1');
    expect(useShell.getState().findTab('t1')).toBeUndefined();
  });
});

describe('R3-05 关闭守卫：批量动作必须续跑', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    vi.spyOn(engine, 'closeSession').mockResolvedValue(null as never);
    vi.spyOn(engine, 'saveText').mockImplementation(async (p: TextSaveParams) =>
      saveResult(p.sessionId, [p.side as 'left' | 'right']),
    );
  });

  it('关闭窗口：两个脏标签逐个确认后窗口真正关闭（此前确认一次就中止）', async () => {
    const winId = addTabs(textTab('t1'), textTab('t2'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    useShell.getState().markContentDirty('t2', { left: false, right: true });

    await useShell.getState().closeWindow(winId);
    expect(useShell.getState().closeGuard?.tabId).toBe('t1');
    await useShell.getState().resolveCloseGuard('discard');

    expect(useShell.getState().closeGuard?.tabId).toBe('t2');
    await useShell.getState().resolveCloseGuard('discard');

    expect(useShell.getState().closeGuard).toBeNull();
    // 最后一个窗口关闭后回到全新主窗口（只剩主页占位），两个脏标签都已关闭
    const win = useShell.getState().windows.find((w) => w.id === winId)!;
    expect(win.tabs.map((t) => t.kind)).toEqual(['home']);
    expect(useShell.getState().findTab('t1')).toBeUndefined();
    expect(useShell.getState().findTab('t2')).toBeUndefined();
  });

  it('关闭其他标签：确认后继续关闭剩余脏标签，保留指定标签', async () => {
    addTabs(textTab('keep'), textTab('t1'), textTab('t2'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    useShell.getState().markContentDirty('t2', { left: true, right: false });

    await useShell.getState().closeOthers('keep');
    expect(useShell.getState().closeGuard?.tabId).toBe('t1');
    await useShell.getState().resolveCloseGuard('save');
    expect(useShell.getState().closeGuard?.tabId).toBe('t2');
    await useShell.getState().resolveCloseGuard('save');

    const tabs = useShell.getState().windows[0].tabs;
    expect(tabs.map((t) => t.id)).toEqual(['home-w-1-main', 'keep']);
    expect(useShell.getState().pendingClose).toBeNull();
  });

  it('取消批量动作 → 不再续跑，剩余标签保持原样', async () => {
    const winId = addTabs(textTab('t1'), textTab('t2'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    useShell.getState().markContentDirty('t2', { left: true, right: false });

    await useShell.getState().closeWindow(winId);
    await useShell.getState().resolveCloseGuard('cancel');

    expect(useShell.getState().pendingClose).toBeNull();
    expect(useShell.getState().closeGuard).toBeNull();
    expect(useShell.getState().findTab('t1')).toBeDefined();
    expect(useShell.getState().findTab('t2')).toBeDefined();
  });

  it('固定标签不参与「关闭其他」的守卫扫描', async () => {
    addTabs(textTab('keep'), textTab('pinned', { pinned: true }));
    useShell.getState().markContentDirty('pinned', { left: true, right: false });

    await useShell.getState().closeOthers('keep');

    expect(useShell.getState().closeGuard).toBeNull();
    expect(useShell.getState().findTab('pinned')).toBeDefined();
  });
});

describe('R3-05 关闭守卫：原生退出', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    vi.spyOn(engine, 'closeSession').mockResolvedValue(null as never);
  });

  it('无未保存项 → 直接调用退出出口', async () => {
    const exit = vi.fn();
    useShell.getState().setExitHandler(exit);
    addTabs(textTab('t1'));

    await useShell.getState().requestAppExit();

    expect(exit).toHaveBeenCalledTimes(1);
    expect(useShell.getState().closeGuard).toBeNull();
  });

  it('有未落盘编辑 → 先提示；处理完最后一个才退出', async () => {
    const exit = vi.fn();
    useShell.getState().setExitHandler(exit);
    addTabs(textTab('t1'), textTab('t2'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });
    useShell.getState().markContentDirty('t2', { left: true, right: false });

    await useShell.getState().requestAppExit();
    expect(exit).not.toHaveBeenCalled();
    expect(useShell.getState().closeGuard).toEqual({ tabId: 't1', kind: 'content' });

    await useShell.getState().resolveCloseGuard('discard');
    expect(exit).not.toHaveBeenCalled();
    expect(useShell.getState().closeGuard?.tabId).toBe('t2');

    await useShell.getState().resolveCloseGuard('discard');
    expect(exit).toHaveBeenCalledTimes(1);
    expect(useShell.getState().pendingClose).toBeNull();
  });

  it('退出提示中取消 → 不退出，标签与编辑保留', async () => {
    const exit = vi.fn();
    useShell.getState().setExitHandler(exit);
    addTabs(textTab('t1'));
    useShell.getState().markContentDirty('t1', { left: true, right: false });

    await useShell.getState().requestAppExit();
    await useShell.getState().resolveCloseGuard('cancel');

    expect(exit).not.toHaveBeenCalled();
    expect(useShell.getState().findTab('t1')!.tab.contentDirty).toEqual({
      left: true,
      right: false,
    });
  });

  it('浏览器演示模式无退出出口时不抛错', async () => {
    addTabs(textTab('t1'));
    await useShell.getState().requestAppExit();
    expect(useShell.getState().closeGuard).toBeNull();
  });
});
