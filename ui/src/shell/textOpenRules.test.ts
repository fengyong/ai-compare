import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { resetShellForTest, useShell } from './store';
import * as engine from '../ipc/engineProcess';
import { sessionService } from '../session/service';
import { getDirCache, releaseDirCache } from '../views/folder-compare/dirRowCache';
import { newTextSessionModel, putTextSession } from '../views/text-compare/textModel';
import type { DirDelta, DirRow, TextOpenParams } from '../protocol/types';
import type { SettingsTree } from '../session/model';
import type { Tab } from './shellState';

/**
 * 壳层预开 text/open 必须携带设置页比较规则（首开即生效）。
 * 视图 open 撞 411 后走 loadTextSessionReuse 直接复用引擎会话，不会重放
 * rules；若壳预开丢 rules，用户配置的重要性口径在首开永远不生效。
 */

const PARENT_SID = 'dir-rules';
const CHILD_SID = 'txt-rules';

const row = (id: string): DirRow => ({
  id,
  name: id,
  kind: 'file',
  left: { size: 1, mtimeMs: 0, unixMode: 0, uid: 0, gid: 0 },
  right: { size: 1, mtimeMs: 0, unixMode: 0, uid: 0, gid: 0 },
  status: 'contentDiff',
  hasChildren: false,
  hidden: false,
  suppressed: false,
  leftPath: id,
  rightPath: id,
});

function seedRules(group: Record<string, unknown> | null): void {
  vi.spyOn(sessionService, 'getKindDefaults').mockReturnValue(
    (group ? { compareRules: group } : {}) as SettingsTree,
  );
  vi.spyOn(sessionService, 'getGlobalSettings').mockReturnValue({
    fileFormats: {},
  } as SettingsTree);
}

function captureOpen(): TextOpenParams[] {
  const opened: TextOpenParams[] = [];
  vi.spyOn(engine, 'openText').mockImplementation(async (p: TextOpenParams) => {
    opened.push(p);
    return {} as never;
  });
  return opened;
}

function seedFolderTab(): void {
  const parent: Tab = {
    id: 'p1',
    kind: 'folder',
    title: 'L ⇄ R',
    folder: { left: '/L', right: '/R', sessionId: PARENT_SID },
  };
  useShell.setState((st) => ({
    windows: st.windows.map((w) => ({
      ...w,
      tabs: [...w.tabs.filter((t) => t.id !== 'p1'), parent],
      activeTabId: 'p1',
    })),
  }));
}

function seedRows(rows: DirRow[]): void {
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
    progress: undefined,
    batch: { index: 0, hasMore: false },
    scanErrors: [],
  };
  getDirCache(PARENT_SID).applyDelta(delta);
}

function seedChildTab(): void {
  const parent: Tab = {
    id: 'p1',
    kind: 'folder',
    title: 'L ⇄ R',
    folder: { left: '/L', right: '/R', sessionId: PARENT_SID },
  };
  const child: Tab = {
    id: 'c1',
    kind: 'text',
    title: 'a.txt（文本比较）',
    text: {
      sessionId: CHILD_SID,
      left: { path: '/L/a.txt', name: 'a.txt' },
      right: { path: '/R/a.txt', name: 'a.txt' },
      parentTabId: 'p1',
      parentFolderSessionId: PARENT_SID,
    },
  };
  useShell.setState((st) => ({
    windows: st.windows.map((w) => ({
      ...w,
      tabs: [...w.tabs.filter((t) => t.id !== 'p1' && t.id !== 'c1'), parent, child],
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

describe('壳层预开 text/open 携带设置规则', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    useShell.setState({ notice: null });
    vi.spyOn(engine, 'closeSession').mockResolvedValue(null as never);
    seedRules({ caseUnimportant: true });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    releaseDirCache(PARENT_SID);
  });

  it('openTextTab：首开即带 compareRules 映射的 rules', async () => {
    const opened = captureOpen();
    await useShell.getState().openTextTab('/L/a.c', '/R/a.c');
    expect(opened).toHaveLength(1);
    expect(opened[0].rules).toMatchObject({ caseUnimportant: true });
  });

  it('openChild（文本子会话）：首开即带 rules', async () => {
    seedFolderTab();
    const opened = captureOpen();
    await useShell.getState().openChild('p1', row('main.c'));
    expect(opened).toHaveLength(1);
    expect(opened[0].rules).toMatchObject({ caseUnimportant: true });
  });

  it('navigateDiffFile：改指下一差异对的首开即带 rules', async () => {
    seedRows([row('a.txt'), row('b.txt')]);
    seedChildTab();
    seedChildModel();
    const opened = captureOpen();
    await useShell.getState().navigateDiffFile('c1', 'next');
    expect(opened).toHaveLength(1);
    expect(opened[0].rules).toMatchObject({ caseUnimportant: true });
  });

  it('未配置 compareRules：rules 为 undefined（引擎默认语义）', async () => {
    seedRules(null);
    const opened = captureOpen();
    await useShell.getState().openTextTab('/L/a.c', '/R/a.c');
    expect(opened[0].rules).toBeUndefined();
  });
});
