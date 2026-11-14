import { beforeEach, describe, expect, it } from 'vitest';
import { activeWindowSelector, resetShellForTest, useShell } from './store';
import { sessionService } from '../session/service';
import type { SessionNode } from '../session/model';
import type { Tab } from './shellState';

/**
 * 命名会话「保存 → 重开」闭环（R3-08 / SES-001 / SES-005）。
 *
 * 此前：merge/merge3 只存 base+right（真实 left 丢失）、picture/table/hex 存空串
 * 且被另存为标成 folder-compare、openSavedSession 只认 4 种类型 →
 * 其余类型重开报"尚未实现"，三路会话重开后 base 被 left 顶替。
 */

function activeTab(): Tab {
  const st = useShell.getState();
  const win = activeWindowSelector(st)!;
  return win.tabs.find((t) => t.id === win.activeTabId)!;
}

/** 另存当前活动标签 → 返回落盘的会话节点 */
async function saveActive(name: string): Promise<SessionNode> {
  const tab = activeTab();
  useShell.getState().saveAsForTab(tab.id);
  await useShell.getState().resolveSaveAs(name);
  const link = activeTab().sessionLink;
  expect(link).toBeDefined();
  return sessionService.getSession(link!.sessionId)!;
}

/** 重开：清空窗口后从会话树加载，返回新活动标签 */
async function reopen(node: SessionNode): Promise<Tab> {
  resetShellForTest();
  await useShell.getState().openSavedSession(node.id);
  const tab = activeTab();
  expect(tab.sessionLink?.sessionId).toBe(node.id);
  return tab;
}

describe('会话保存 → 重开：两路会话', () => {
  beforeEach(() => resetShellForTest());

  it('文件夹比较：类型/路径/关联原样往返', async () => {
    useShell.getState().openFolderTab('/L', '/R');
    const node = await saveActive('往返-目录');
    expect(node.kind).toBe('folder-compare');
    expect(node.specs).toEqual({ left: '/L', right: '/R' });

    const tab = await reopen(node);
    expect(tab.kind).toBe('folder');
    expect(tab.folder).toMatchObject({ left: '/L', right: '/R' });
  });

  it('文件夹同步：重开为 sync 标签而非目录比较', async () => {
    useShell.getState().openFolderSyncTab('/L', '/R');
    const node = await saveActive('往返-同步');
    expect(node.kind).toBe('folder-sync');

    const tab = await reopen(node);
    expect(tab.kind).toBe('sync');
    expect(tab.sync).toMatchObject({ left: '/L', right: '/R' });
  });

  it.each([
    ['picture', 'picture-compare', '/a.png', '/b.png'],
    ['table', 'table-compare', '/a.csv', '/b.csv'],
    ['hex', 'hex-compare', '/a.bin', '/b.bin'],
  ] as const)(
    '%s：另存为落真实类型与真实路径，重开同类型',
    async (kind, sessionKind, left, right) => {
      const st = useShell.getState();
      if (kind === 'picture') st.openPictureTab(left, right);
      else if (kind === 'table') st.openTableTab(left, right);
      else st.openHexTab(left, right);

      const node = await saveActive(`往返-${kind}`);
      // 回归锚点：此前这三类被标成 folder-compare 且 specs 为空串
      expect(node.kind).toBe(sessionKind);
      expect(node.specs).toEqual({ left, right });

      const tab = await reopen(node);
      expect(tab.kind).toBe(kind);
      expect(tab[kind]).toMatchObject({ left, right });
      // 重开记入该类型的 MRU（类型标错时这里会落到 folder-compare）
      expect(sessionService.getMruPaths(sessionKind).left).toContain(left);
    },
  );
});

describe('会话保存 → 重开：三路会话必须保住 base/left/right', () => {
  beforeEach(() => resetShellForTest());

  it('文件夹合并：base 与 left 不同，重开后两者都不丢', async () => {
    useShell.getState().openFolderMergeTab('/B', '/L', '/R');
    const node = await saveActive('往返-目录合并');
    expect(node.kind).toBe('folder-merge');
    expect(node.specs).toEqual({ base: '/B', left: '/L', right: '/R' });

    const tab = await reopen(node);
    expect(tab.kind).toBe('merge');
    expect(tab.merge).toMatchObject({ base: '/B', left: '/L', right: '/R' });
  });

  it('三向文本合并：重开为 merge3 标签（此前报"尚未实现"）', async () => {
    useShell.getState().openMerge3Tab('/B.txt', '/L.txt', '/R.txt');
    const node = await saveActive('往返-文本合并');
    expect(node.kind).toBe('text-merge');
    expect(node.specs).toEqual({ base: '/B.txt', left: '/L.txt', right: '/R.txt' });

    const tab = await reopen(node);
    expect(tab.kind).toBe('merge3');
    expect(tab.merge3).toMatchObject({ base: '/B.txt', left: '/L.txt', right: '/R.txt' });
  });

  it('旧会话文件只有 left/right 两位：缺 base 时以 left 兼作（不抛错）', async () => {
    const legacy = await sessionService.saveSession({
      kind: 'folder-merge',
      name: '旧版两路位',
      specs: { left: '/L', right: '/R' },
    });
    expect(legacy.specs.base).toBeUndefined();

    const tab = await reopen(legacy);
    expect(tab.merge).toMatchObject({ base: '/L', left: '/L', right: '/R' });
  });
});

describe('会话保存 → 重开：文本比较', () => {
  beforeEach(() => resetShellForTest());

  it('文本比较：路径往返且重开为 text 标签', async () => {
    await useShell.getState().openTextTab('/L.txt', '/R.txt');
    const node = await saveActive('往返-文本');
    expect(node.kind).toBe('text-compare');
    expect(node.specs).toEqual({ left: '/L.txt', right: '/R.txt' });

    const tab = await reopen(node);
    expect(tab.kind).toBe('text');
    expect(tab.text?.left.path).toBe('/L.txt');
    expect(tab.text?.right.path).toBe('/R.txt');
  });
});
