import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { resetShellForTest, useShell } from './store';
import { commandRegistry, type CommandContext } from './commands';
import { commandEnabled, groupCommands } from './menuModel';
import * as engine from '../ipc/engineProcess';
import { getDirCache, releaseDirCache } from '../views/folder-compare/dirRowCache';
import {
  getTextSession,
  newTextSessionModel,
  putTextSession,
} from '../views/text-compare/textModel';
import { S } from '../strings';
import type {
  DirDelta,
  DirExecuteParams,
  DirExecuteResult,
  DirRow,
  TextOpenParams,
} from '../protocol/types';
import type { Tab } from './shellState';

/**
 * FR-DIR-071 子会话差异文件遍历的接线回归。
 *
 * 纯遍历序/回绕在 diffFileNav.test.ts；这里盯的是壳层编排：改指标签而非新开标签、
 * 旧会话清理、未落盘编辑守卫、复制确认与失败不跳转、目标在复制前算定。
 */

const PARENT_SID = 'dir-1';
const CHILD_SID = 'txt-1';

const row = (id: string, over: Partial<DirRow> = {}): DirRow => ({
  id,
  name: id,
  kind: 'file',
  left: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 },
  right: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 },
  status: 'contentDiff',
  hasChildren: false,
  hidden: false,
  suppressed: false,
  leftPath: id,
  rightPath: id,
  ...over,
});

function seedRows(rows: DirRow[]): void {
  // upsert 只增改不删：每次播种都从空缓存开始，否则上一用例的行会参与遍历
  releaseDirCache(PARENT_SID);
  const delta: DirDelta = {
    sessionId: PARENT_SID,
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
    progress: null,
    batch: { index: 0, hasMore: false },
    scanErrors: [],
  };
  getDirCache(PARENT_SID).applyDelta(delta);
}

function seedTabs(current = 'a.txt'): void {
  const parent: Tab = {
    id: 'p1',
    kind: 'folder',
    title: 'L ⇄ R',
    folder: { left: '/L', right: '/R', sessionId: PARENT_SID },
  };
  const child: Tab = {
    id: 'c1',
    kind: 'text',
    title: `${current}（文本比较）`,
    text: {
      sessionId: CHILD_SID,
      left: { path: `/L/${current}`, name: current },
      right: { path: `/R/${current}`, name: current },
      parentTabId: 'p1',
      parentFolderSessionId: PARENT_SID,
    },
  };
  const plain: Tab = {
    id: 't-plain',
    kind: 'text',
    title: '独立文本比较',
    text: {
      sessionId: 'txt-plain',
      left: { path: '/L/a.txt', name: 'a.txt' },
      right: { path: '/R/a.txt', name: 'a.txt' },
    },
  };
  useShell.setState((st) => ({
    windows: st.windows.map((w) => ({
      ...w,
      tabs: [
        ...w.tabs.filter((t) => t.id !== 'p1' && t.id !== 'c1' && t.id !== 't-plain'),
        parent,
        child,
        plain,
      ],
      activeTabId: 'c1',
    })),
  }));
}

function seedChildModel(): void {
  putTextSession(
    newTextSessionModel({
      sessionId: CHILD_SID,
      leftPath: '/L/a.txt',
      rightPath: '/R/a.txt',
      leftName: 'a.txt',
      rightName: 'a.txt',
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
    }),
  );
}

function execResult(ops: DirExecuteParams['ops'], failed = false): DirExecuteResult {
  const item = (status: 'succeeded' | 'failed') =>
    ops.map((op) => ({ op, status, detail: status === 'failed' ? '磁盘只读' : null }));
  return {
    sessionId: PARENT_SID,
    total: ops.length,
    succeeded: failed ? [] : item('succeeded'),
    failed: failed ? item('failed') : [],
    skipped: [],
    dryRun: false,
    cancelled: false,
  };
}

function childText() {
  return useShell.getState().findTab('c1')!.tab.text!;
}

describe('navigateDiffFile（FR-DIR-071）', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    // resetShellForTest 不清 notice：留着上一用例的提示会让"未提示"断言假绿
    useShell.setState({ notice: null });
    vi.spyOn(engine, 'closeSession').mockResolvedValue(null as never);
    vi.spyOn(engine, 'openText').mockResolvedValue({} as never);
    seedRows([row('a.txt'), row('b.txt'), row('c.txt'), row('same.txt', { status: 'same' })]);
    seedTabs();
  });

  afterEach(() => {
    releaseDirCache(PARENT_SID);
  });

  it('next：改指当前标签到下一差异对（不新开标签），父链接保留', async () => {
    const opened: TextOpenParams[] = [];
    vi.spyOn(engine, 'openText').mockImplementation(async (p: TextOpenParams) => {
      opened.push(p);
      return {} as never;
    });
    const tabsBefore = useShell.getState().windows[0].tabs.length;

    await useShell.getState().navigateDiffFile('c1', 'next');

    const text = childText();
    expect(text.left).toEqual({ path: '/L/b.txt', name: 'b.txt' });
    expect(text.right).toEqual({ path: '/R/b.txt', name: 'b.txt' });
    expect(text.parentTabId).toBe('p1');
    expect(text.parentFolderSessionId).toBe(PARENT_SID);
    expect(text.sessionId).not.toBe(CHILD_SID);
    expect(useShell.getState().findTab('c1')!.tab.title).toContain('b.txt');
    expect(useShell.getState().windows[0].tabs).toHaveLength(tabsBefore);
    expect(opened).toHaveLength(1);
    expect(opened[0].left).toBe('/L/b.txt');
    expect(opened[0].sessionId).toBe(text.sessionId);
  });

  it('prev：首个差异对回绕到最后一个', async () => {
    await useShell.getState().navigateDiffFile('c1', 'prev');
    expect(childText().left.path).toBe('/L/c.txt');
  });

  it('父视图显示过滤把目录标 hidden 时，子树里的差异文件仍是跳转目标', async () => {
    seedRows([
      row('a.txt'),
      row('sub', { kind: 'dir', status: 'dirSame', hidden: true }),
      row('sub/deep.txt', { hidden: true }),
      row('z.txt'),
    ]);

    await useShell.getState().navigateDiffFile('c1', 'prev');

    expect(childText().left.path).toBe('/L/sub/deep.txt');
  });

  it('旧会话按关闭标签同一套清理：句柄注销 + 模型缓存释放 + session/close', async () => {
    seedChildModel();
    const untrack = vi.spyOn(engine, 'untrackSession');
    expect(getTextSession(CHILD_SID)).toBeDefined();

    await useShell.getState().navigateDiffFile('c1', 'next');

    expect(untrack).toHaveBeenCalledWith(CHILD_SID);
    expect(engine.closeSession).toHaveBeenCalledWith({ sessionId: CHILD_SID });
    expect(getTextSession(CHILD_SID)).toBeUndefined();
  });

  it('未落盘编辑：拒绝跳转并提示，标签与引擎会话都不动（R3-05 同口径）', async () => {
    useShell.getState().markContentDirty('c1', { left: true, right: false });

    await useShell.getState().navigateDiffFile('c1', 'next');

    expect(useShell.getState().notice).toBe(S.text.navDirty);
    expect(childText().sessionId).toBe(CHILD_SID);
    expect(childText().left.path).toBe('/L/a.txt');
    expect(engine.openText).not.toHaveBeenCalled();
    expect(engine.closeSession).not.toHaveBeenCalled();
  });

  it('父会话没有差异文件：提示且不跳转', async () => {
    seedRows([row('a.txt', { status: 'same' })]);

    await useShell.getState().navigateDiffFile('c1', 'next');

    expect(useShell.getState().notice).toBe(S.text.noDiffFiles);
    expect(childText().left.path).toBe('/L/a.txt');
  });

  it('非子会话（无父链接）：命令无效果', async () => {
    useShell.setState((st) => ({
      windows: st.windows.map((w) => ({
        ...w,
        tabs: w.tabs.map((t) =>
          t.id === 'c1' && t.text
            ? {
                ...t,
                text: { sessionId: t.text.sessionId, left: t.text.left, right: t.text.right },
              }
            : t,
        ),
      })),
    }));

    await useShell.getState().navigateDiffFile('c1', 'next');

    expect(childText().left.path).toBe('/L/a.txt');
    expect(engine.openText).not.toHaveBeenCalled();
  });

  it('复制到右侧并打开下一个：先确认 → dir/execute 整文件复制 → 回传父会话 → 跳转', async () => {
    const executed: DirExecuteParams[] = [];
    vi.spyOn(engine, 'executeOperations').mockImplementation(async (p: DirExecuteParams) => {
      executed.push(p);
      return execResult(p.ops);
    });
    const filtered: unknown[] = [];
    vi.spyOn(engine, 'filterDir').mockImplementation(async (p) => {
      filtered.push(p);
      return {} as never;
    });

    const pending = useShell.getState().navigateDiffFile('c1', 'next', 'right');
    expect(useShell.getState().confirm?.title).toBe(S.text.navCopyTitle);
    useShell.getState().resolveConfirm(true);
    await pending;

    expect(executed).toHaveLength(1);
    expect(executed[0].sessionId).toBe(PARENT_SID);
    expect(executed[0].ops).toEqual([
      { kind: 'copy', path: 'a.txt', side: 'left', targetSide: 'right' },
    ]);
    expect(filtered).toEqual([
      {
        sessionId: PARENT_SID,
        nameFilter: null,
        otherFilter: null,
        displayFilter: null,
        suppress: null,
        flat: null,
        rescan: true,
      },
    ]);
    expect(childText().left.path).toBe('/L/b.txt');
  });

  it('复制到左侧：源侧取右（方向语义 FR-DIR-060）', async () => {
    const executed: DirExecuteParams[] = [];
    vi.spyOn(engine, 'executeOperations').mockImplementation(async (p: DirExecuteParams) => {
      executed.push(p);
      return execResult(p.ops);
    });

    const pending = useShell.getState().navigateDiffFile('c1', 'next', 'left');
    useShell.getState().resolveConfirm(true);
    await pending;

    expect(executed[0].ops).toEqual([
      { kind: 'copy', path: 'a.txt', side: 'right', targetSide: 'left' },
    ]);
  });

  it('确认框取消：不复制也不跳转', async () => {
    const executed = vi.spyOn(engine, 'executeOperations');

    const pending = useShell.getState().navigateDiffFile('c1', 'next', 'right');
    useShell.getState().resolveConfirm(false);
    await pending;

    expect(executed).not.toHaveBeenCalled();
    expect(childText().left.path).toBe('/L/a.txt');
  });

  it('复制失败：提示失败原因且停在当前文件（不得跳走后丢掉失败现场）', async () => {
    vi.spyOn(engine, 'executeOperations').mockImplementation(async (p: DirExecuteParams) =>
      execResult(p.ops, true),
    );

    const pending = useShell.getState().navigateDiffFile('c1', 'next', 'right');
    useShell.getState().resolveConfirm(true);
    await pending;

    expect(useShell.getState().notice).toContain('磁盘只读');
    expect(childText().left.path).toBe('/L/a.txt');
    expect(engine.openText).not.toHaveBeenCalled();
  });

  it('只剩当前一个差异对：复制照做，随后提示没有更多并停在原文件', async () => {
    seedRows([row('a.txt')]);
    const executed = vi
      .spyOn(engine, 'executeOperations')
      .mockImplementation(async (p: DirExecuteParams) => execResult(p.ops));

    const pending = useShell.getState().navigateDiffFile('c1', 'next', 'right');
    useShell.getState().resolveConfirm(true);
    await pending;

    expect(executed).toHaveBeenCalledTimes(1);
    expect(useShell.getState().notice).toBe(S.text.noMoreDiffFiles);
    expect(childText().left.path).toBe('/L/a.txt');
  });

  it('目标在复制之前算定：复制使当前对判相同后仍按原位次前进', async () => {
    seedTabs('b.txt');
    vi.spyOn(engine, 'executeOperations').mockImplementation(async (p: DirExecuteParams) => {
      // 复制 + 回传的真实后果：当前对从差异列表消失
      seedRows([row('a.txt'), row('b.txt', { status: 'same' }), row('c.txt')]);
      return execResult(p.ops);
    });

    const pending = useShell.getState().navigateDiffFile('c1', 'prev', 'right');
    useShell.getState().resolveConfirm(true);
    await pending;

    expect(childText().left.path).toBe('/L/a.txt');
  });

  it('新会话打开失败：保留当前标签与旧会话，只提示', async () => {
    vi.spyOn(engine, 'openText').mockRejectedValue(new Error('文件不可读'));

    await useShell.getState().navigateDiffFile('c1', 'next');

    expect(useShell.getState().notice).toContain('文件不可读');
    expect(childText().sessionId).toBe(CHILD_SID);
    expect(childText().left.path).toBe('/L/a.txt');
    expect(engine.closeSession).not.toHaveBeenCalled();
  });
});

const NAV_IDS = [
  'search.nextDiffFile',
  'search.prevDiffFile',
  'search.copyToRightNext',
  'search.copyToLeftNext',
] as const;

function ctx(
  activeTabKind: CommandContext['activeTabKind'],
  activeTabId: string | null,
): CommandContext {
  return {
    sessions: {} as never,
    clipboard: {} as never,
    notice: () => {},
    activeTabKind,
    activeTabId,
  };
}

describe('search.* 差异文件遍历命令（FR-DIR-071 / 附录 F）', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    useShell.setState({ notice: null });
    seedRows([row('a.txt'), row('b.txt')]);
    seedTabs();
  });

  afterEach(() => {
    releaseDirCache(PARENT_SID);
  });

  it('四条命令都注册在搜索菜单、无占位阶段注记', () => {
    const groups = groupCommands(commandRegistry.list(), ctx('text', 'c1'));
    const search = groups.find((g) => g.id === 'search')!;
    // 菜单内按标题排序，故比集合而非注册顺序
    expect(search.entries.map((e) => e.def.id).sort()).toEqual([...NAV_IDS].sort());
    for (const id of NAV_IDS) {
      const def = commandRegistry.get(id)!;
      expect(def.category).toBe('搜索');
      expect(def.phaseNote).toBeUndefined();
    }
  });

  it('门控：仅文件夹子会话可用（独立文本标签/目录视图/主页都灰显）', () => {
    for (const id of NAV_IDS) {
      const def = commandRegistry.get(id)!;
      expect(commandEnabled(def, ctx('text', 'c1'))).toBe(true);
      expect(commandEnabled(def, ctx('text', 't-plain'))).toBe(false);
      expect(commandEnabled(def, ctx('folder', 'p1'))).toBe(false);
      expect(commandEnabled(def, ctx('home', null))).toBe(false);
    }
  });

  it('run 真的调到壳层遍历（方向与复制侧正确，不是占位桩）', async () => {
    const spy = vi.spyOn(useShell.getState(), 'navigateDiffFile').mockResolvedValue(undefined);
    const c = ctx('text', 'c1');

    await commandRegistry.get('search.nextDiffFile')!.run(c);
    await commandRegistry.get('search.prevDiffFile')!.run(c);
    await commandRegistry.get('search.copyToRightNext')!.run(c);
    await commandRegistry.get('search.copyToLeftNext')!.run(c);

    expect(spy.mock.calls).toEqual([
      ['c1', 'next', undefined],
      ['c1', 'prev', undefined],
      ['c1', 'next', 'right'],
      ['c1', 'next', 'left'],
    ]);
  });
});
