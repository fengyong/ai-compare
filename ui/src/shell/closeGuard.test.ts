import { describe, expect, it } from 'vitest';
import {
  closeGuardKind,
  firstPendingGuard,
  guardOf,
  hasContentEdits,
  remainingSides,
} from './closeGuard';
import type { Tab, WindowState } from './shellState';

const text = (over: Partial<Tab> = {}): Tab => ({
  id: 't1',
  kind: 'text',
  title: 'x',
  text: { sessionId: 's1', left: { path: '/l', name: 'l' }, right: { path: '/r', name: 'r' } },
  ...over,
});

const link = { sessionId: 'ns', name: '命名', kind: 'text-compare' as const, locked: false };

describe('关闭守卫判定（R3-05）', () => {
  it('未命名标签的文件编辑同样触发守卫（此前只看 sessionLink）', () => {
    expect(closeGuardKind(text({ contentDirty: { left: true, right: false } }))).toBe('content');
    expect(closeGuardKind(text({ contentDirty: { left: false, right: true } }))).toBe('content');
    expect(guardOf(text({ contentDirty: { left: true, right: true } }))).toEqual({
      tabId: 't1',
      kind: 'content',
    });
  });

  it('两侧都已落盘 / 从未编辑 → 无守卫', () => {
    expect(closeGuardKind(text())).toBeNull();
    expect(closeGuardKind(text({ contentDirty: { left: false, right: false } }))).toBeNull();
    expect(hasContentEdits(text())).toBe(false);
  });

  it('文件编辑优先于命名会话参数（丢数据 > 丢视图状态）', () => {
    const both = text({
      sessionLink: link,
      dirty: true,
      contentDirty: { left: true, right: false },
    });
    expect(closeGuardKind(both)).toBe('content');
    expect(closeGuardKind(text({ sessionLink: link, dirty: true }))).toBe('session');
  });

  it('命名会话脏但无 sessionLink 不构成守卫；home 永不守卫', () => {
    expect(closeGuardKind(text({ dirty: true }))).toBeNull();
    expect(
      closeGuardKind({
        id: 'h',
        kind: 'home',
        title: '主页',
        dirty: true,
        contentDirty: { left: true, right: true },
      }),
    ).toBeNull();
  });

  it('firstPendingGuard 按窗口/标签顺序取第一个', () => {
    const clean: WindowState = { id: 'w1', title: '', activeTabId: 'a', tabs: [text({ id: 'a' })] };
    const dirty: WindowState = {
      id: 'w2',
      title: '',
      activeTabId: 'c',
      tabs: [text({ id: 'b' }), text({ id: 'c', contentDirty: { left: false, right: true } })],
    };
    expect(firstPendingGuard([clean, dirty])).toEqual({ tabId: 'c', kind: 'content' });
    expect(firstPendingGuard([clean])).toBeNull();
    expect(firstPendingGuard([])).toBeNull();
  });

  it('remainingSides 只清除真正落盘的侧', () => {
    const dirty = { left: true, right: true };
    expect(remainingSides(dirty, [{ side: 'left' }])).toEqual({ left: false, right: true });
    expect(remainingSides(dirty, [])).toEqual({ left: true, right: true });
    expect(remainingSides(dirty, [{ side: 'left' }, { side: 'right' }])).toEqual({
      left: false,
      right: false,
    });
    expect(dirty).toEqual({ left: true, right: true });
  });
});
