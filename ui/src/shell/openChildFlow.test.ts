import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { resetShellForTest, useShell } from './store';
import * as engine from '../ipc/engineProcess';
import { sessionService } from '../session/service';
import { S } from '../strings';
import type { DirRow, TextOpenParams } from '../protocol/types';
import type { SettingsTree } from '../session/model';
import type { Tab } from './shellState';

/**
 * FR-DIR-070 / FR-SEL-001 双击目录行的选型接线回归。
 *
 * 纯掩码/首匹配语义在 settings/formatsModel.test.ts；这里盯壳层编排：
 * 视图类型分派、两侧地址取 leftPath/rightPath、子标签落在父窗口、
 * 设置层格式覆盖参与选型、外部格式与缺路径的提示。
 */

const PARENT_SID = 'dir-open-child';

const row = (name: string, over: Partial<DirRow> = {}): DirRow => ({
  id: name,
  name,
  kind: 'file',
  left: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 },
  right: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 },
  status: 'contentDiff',
  hasChildren: false,
  hidden: false,
  suppressed: false,
  leftPath: name,
  rightPath: name,
  ...over,
});

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

/** 设置层格式组（spy 掉 IO：选型只依赖生效格式列表） */
function mockFormats(group: unknown): void {
  vi.spyOn(sessionService, 'getGlobalSettings').mockReturnValue({
    fileFormats: group,
  } as SettingsTree);
}

function lastTab(): Tab {
  const w = useShell.getState().windows[0];
  return w.tabs[w.tabs.length - 1];
}

function tabCount(): number {
  return useShell.getState().windows.reduce((n, w) => n + w.tabs.length, 0);
}

describe('openChild（FR-DIR-070 双击自动选型）', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
    useShell.setState({ notice: null });
    vi.spyOn(engine, 'openText').mockResolvedValue({} as never);
    mockFormats({});
    seedFolderTab();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('csv → 表格视图：路径由两侧根 + 相对路径拼出，落在父窗口并激活', async () => {
    await useShell.getState().openChild('p1', row('data.csv'));

    const tab = lastTab();
    expect(tab.kind).toBe('table');
    expect(tab.table).toMatchObject({ left: '/L/data.csv', right: '/R/data.csv' });
    expect(tab.table!.sessionId).toBeTruthy();
    expect(useShell.getState().windows[0].activeTabId).toBe(tab.id);
    expect(engine.openText).not.toHaveBeenCalled();
  });

  it('png → 图片视图（FR-PIX-001 内置关联）', async () => {
    await useShell.getState().openChild('p1', row('shot.png'));
    expect(lastTab().kind).toBe('picture');
    expect(lastTab().picture).toMatchObject({ left: '/L/shot.png', right: '/R/shot.png' });
  });

  it('dll → 十六进制视图', async () => {
    await useShell.getState().openChild('p1', row('lib.dll'));
    expect(lastTab().kind).toBe('hex');
    expect(lastTab().hex).toMatchObject({ left: '/L/lib.dll', right: '/R/lib.dll' });
  });

  it('无匹配格式且非文本扩展名 → 十六进制兜底（FR-SEL-001 内部启发）', async () => {
    await useShell.getState().openChild('p1', row('blob.xyz'));
    expect(lastTab().kind).toBe('hex');
  });

  it('源码 → 文本子会话：记录父目录链接并调 text/open', async () => {
    const opened: TextOpenParams[] = [];
    vi.spyOn(engine, 'openText').mockImplementation(async (p: TextOpenParams) => {
      opened.push(p);
      return {} as never;
    });

    await useShell.getState().openChild('p1', row('main.c'));

    const tab = lastTab();
    expect(tab.kind).toBe('text');
    expect(tab.text!.parentTabId).toBe('p1');
    expect(tab.text!.parentFolderSessionId).toBe(PARENT_SID);
    expect(tab.text!.left).toEqual({ path: '/L/main.c', name: 'main.c' });
    expect(opened).toHaveLength(1);
    expect(opened[0]).toMatchObject({ left: '/L/main.c', right: '/R/main.c' });
  });

  it('无扩展名（Makefile）→ 文本视图', async () => {
    await useShell.getState().openChild('p1', row('Makefile'));
    expect(lastTab().kind).toBe('text');
  });

  it('平铺模式 id 是显示名：地址仍取 leftPath/rightPath（R3-03）', async () => {
    await useShell
      .getState()
      .openChild(
        'p1',
        row('data.csv', { id: 'data.csv#1', leftPath: 'sub/data.csv', rightPath: 'sub/data.csv' }),
      );
    expect(lastTab().table).toMatchObject({ left: '/L/sub/data.csv', right: '/R/sub/data.csv' });
  });

  it('手动对齐左右不同名：两侧各用自己的相对路径', async () => {
    await useShell
      .getState()
      .openChild('p1', row('a.csv', { leftPath: 'a.csv', rightPath: 'b.csv' }));
    expect(lastTab().table).toMatchObject({ left: '/L/a.csv', right: '/R/b.csv' });
  });

  it('用户自定义格式参与选型：*.log 指到图片视图（覆盖文本兜底）', async () => {
    mockFormats({
      custom: [
        {
          id: 'fmt-log-pic',
          enabled: true,
          masks: ['*.log'],
          description: '',
          viewKind: 'picture',
          grammar: [],
          tabStops: 4,
          independentLines: false,
          columnData: false,
        },
      ],
    });

    await useShell.getState().openChild('p1', row('trace.log'));
    expect(lastTab().kind).toBe('picture');
  });

  it('内置图片格式被禁用 → png 回落十六进制（不再硬编码扩展名表）', async () => {
    mockFormats({ builtinOverrides: { 'builtin-picture': { enabled: false } } });

    await useShell.getState().openChild('p1', row('shot.png'));
    expect(lastTab().kind).toBe('hex');
  });

  it('列表顺序即优先级：自定义 *.csv→hex 追加在内置表格之后不生效', async () => {
    mockFormats({
      custom: [
        {
          id: 'fmt-csv-hex',
          enabled: true,
          masks: ['*.csv'],
          description: '',
          viewKind: 'hex',
          grammar: [],
          tabStops: 4,
          independentLines: false,
          columnData: false,
        },
      ],
    });

    await useShell.getState().openChild('p1', row('data.csv'));
    expect(lastTab().kind).toBe('table');
  });

  it('外部格式：给出提示且不开标签（FR-FMT-005 尚未实现）', async () => {
    mockFormats({
      custom: [
        {
          id: 'fmt-ext',
          enabled: true,
          masks: ['*.cad'],
          description: '',
          viewKind: 'external',
          grammar: [],
          tabStops: 4,
          independentLines: false,
          columnData: false,
        },
      ],
    });
    const before = tabCount();

    await useShell.getState().openChild('p1', row('part.cad'));

    expect(tabCount()).toBe(before);
    expect(useShell.getState().notice).toBe(S.text.externalView);
  });

  it('缺一侧路径：提示且不开标签', async () => {
    const before = tabCount();
    await useShell.getState().openChild('p1', row('a.csv', { rightPath: undefined }));
    expect(tabCount()).toBe(before);
    expect(useShell.getState().notice).toBe(S.text.unsupportedView);
  });

  it('父标签不存在或不是目录比较：静默返回', async () => {
    const before = tabCount();
    await useShell.getState().openChild('nope', row('a.csv'));
    await useShell.getState().openChild(useShell.getState().windows[0].tabs[0].id, row('a.csv'));
    expect(tabCount()).toBe(before);
    expect(useShell.getState().notice).toBeNull();
  });

  it('多窗口：子标签开在父目录所在窗口，不串到当前活动窗口', async () => {
    useShell.getState().openNewWindow({ withHomeTab: false });
    const secondId = useShell.getState().windows[1].id;
    // 把父目录标签移到第二个窗口，活动窗口仍留在第一个
    useShell.setState((st) => ({
      windows: st.windows.map((w, i) =>
        i === 0
          ? { ...w, tabs: w.tabs.filter((t) => t.id !== 'p1'), activeTabId: w.tabs[0].id }
          : {
              ...w,
              tabs: [...w.tabs, st.windows[0].tabs.find((t) => t.id === 'p1')!],
              activeTabId: 'p1',
            },
      ),
      activeWindowId: st.windows[0].id,
    }));

    await useShell.getState().openChild('p1', row('data.csv'));

    const w2 = useShell.getState().windows.find((w) => w.id === secondId)!;
    expect(w2.tabs[w2.tabs.length - 1].kind).toBe('table');
    expect(useShell.getState().windows[0].tabs.some((t) => t.kind === 'table')).toBe(false);
    expect(useShell.getState().activeWindowId).toBe(secondId);
  });
});
