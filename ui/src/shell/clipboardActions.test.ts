import { beforeEach, describe, expect, it, vi } from 'vitest';
import { clipboardStore } from '../lib/clipboardStore';
import { S } from '../strings';
import {
  captureClipboard,
  compareClipboardEntry,
  compareLatestClipboard,
  comparePathWithClipboard,
} from './clipboardActions';
import { activeWindowSelector, bootstrapShell, resetShellForTest, useShell } from './store';

const notices: string[] = [];
const notice = (m: string) => notices.push(m);
const gateway = (text: string) => ({
  readText: async () => text,
  writeText: async () => {},
});

beforeEach(() => {
  resetShellForTest();
  vi.restoreAllMocks();
  clipboardStore.clear();
  notices.length = 0;
  bootstrapShell();
});

describe('剪贴板比较动作（FR-CLIP-002/004）', () => {
  it('历史不足两条时提示且不打开标签', async () => {
    await compareLatestClipboard(notice);
    expect(notices).toContain(S.clip.needTwo);
    expect(activeWindowSelector(useShell.getState())!.tabs).toHaveLength(1);
  });

  it('capture 两次后最近两条比较打开 text 标签（较旧在左、最新在右）', async () => {
    await captureClipboard(gateway('older\nshared\n'));
    await captureClipboard(gateway('newer\nshared\n'));
    await compareLatestClipboard(notice);
    const win = activeWindowSelector(useShell.getState())!;
    expect(win.tabs).toHaveLength(2);
    const tab = win.tabs[1];
    expect(tab.kind).toBe('text');
    expect(tab.text?.left.path).toMatch(/^clipboard:\/\/\d+$/);
    expect(tab.text?.right.path).toMatch(/^clipboard:\/\/\d+$/);
    expect(tab.text?.left.path).not.toBe(tab.text?.right.path);
  });

  it('与剪贴板比较：左侧保持文件路径、右侧为剪贴板规格', async () => {
    await comparePathWithClipboard('/tmp/left.c', gateway('clip text\n'), notice);
    const tab = activeWindowSelector(useShell.getState())!.tabs[1];
    expect(tab.text?.left.path).toBe('/tmp/left.c');
    expect(tab.text?.right.path).toMatch(/^clipboard:\/\//);
  });

  it('剪贴板为空时提示且不打开标签', async () => {
    await comparePathWithClipboard('/tmp/left.c', gateway(''), notice);
    expect(notices).toContain(S.clip.empty);
    expect(activeWindowSelector(useShell.getState())!.tabs).toHaveLength(1);
  });

  it('面板单条比较取相邻条目', async () => {
    clipboardStore.add('a\n');
    clipboardStore.add('b\n');
    clipboardStore.add('c\n');
    const mid = clipboardStore.list()[1].id;
    await compareClipboardEntry(mid, notice);
    expect(activeWindowSelector(useShell.getState())!.tabs).toHaveLength(2);
  });

  it('面板开关由 UiSlice 驱动', () => {
    expect(useShell.getState().clipboardPanelOpen).toBe(false);
    useShell.getState().toggleClipboardPanel(true);
    expect(useShell.getState().clipboardPanelOpen).toBe(true);
    useShell.getState().toggleClipboardPanel();
    expect(useShell.getState().clipboardPanelOpen).toBe(false);
  });
});
