import { describe, expect, it } from 'vitest';
import {
  activeTextView,
  registerTextView,
  resetTextViewBusForTest,
  setActiveTextView,
  type TextViewActions,
} from './textViewBus';
import {
  BOOKMARK_SLOTS,
  bookmarkLabel,
  bookmarkSlotAtLine,
  bookmarkTargets,
  clearBookmark,
  clearBookmarks,
  nextBookmarkLine,
  prevBookmarkLine,
  setBookmark,
  toggleBookmark,
} from './bookmarks';

describe('文本书签（FR-TXT-023）', () => {
  it('编号常量为 0-9 十个', () => {
    expect(BOOKMARK_SLOTS).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it('切换：当前行无书签时分配最小空闲编号，重复切换清除', () => {
    const r1 = toggleBookmark({}, 12);
    expect(r1).toMatchObject({ slot: 0, added: true });
    expect(r1.map[0]).toBe(12);
    const r2 = toggleBookmark(r1.map, 20);
    expect(r2).toMatchObject({ slot: 1, added: true });
    const r3 = toggleBookmark(r2.map, 12);
    expect(r3).toMatchObject({ slot: 0, added: false });
    expect(r3.map[0]).toBeUndefined();
    expect(r3.map[1]).toBe(20);
  });

  it('十个编号全满且当前行无书签时不新增', () => {
    const full = Object.fromEntries(BOOKMARK_SLOTS.map((s) => [s, s * 10])) as Record<number, number>;
    const r = toggleBookmark(full, 999);
    expect(r.slot).toBeNull();
    expect(r.added).toBe(false);
    expect(Object.keys(r.map)).toHaveLength(10);
  });

  it('指定编号放置、清除、全部清除', () => {
    const m = setBookmark(setBookmark({}, 3, 7), 9, 2);
    expect(bookmarkSlotAtLine(m, 7)).toBe(3);
    expect(bookmarkSlotAtLine(m, 8)).toBeNull();
    expect(clearBookmark(m, 3)[3]).toBeUndefined();
    expect(clearBookmarks()).toEqual({});
    expect(setBookmark({}, 42, 1)).toEqual({});
  });

  it('目标按行号排序；转到下一/上一书签支持循环', () => {
    const m = { 0: 30, 1: 5, 2: 18 };
    expect(bookmarkTargets(m).map((t) => t.line)).toEqual([5, 18, 30]);
    expect(nextBookmarkLine(m, 5)).toBe(18);
    expect(nextBookmarkLine(m, 18)).toBe(30);
    expect(nextBookmarkLine(m, 30)).toBe(5);
    expect(prevBookmarkLine(m, 30)).toBe(18);
    expect(prevBookmarkLine(m, 5)).toBe(30);
    expect(nextBookmarkLine({}, 0)).toBeNull();
    expect(prevBookmarkLine({}, 0)).toBeNull();
  });

  it('标签显示 1 起行号', () => {
    expect(bookmarkLabel({})).toBe('');
    expect(bookmarkLabel({ 0: 0, 3: 11 })).toBe('0:1 3:12');
  });
});

describe('文本视图命令总线', () => {
  it('注册与注销动作（视图卸载时清空）', () => {
    let hits = 0;
    const actions: TextViewActions = {
      toggleWrap: () => { hits++; },
      toggleBookmark: () => { hits++; },
      gotoBookmark: () => { hits++; },
      clearBookmarks: () => { hits++; },
      setBookmarkAtCursor: () => { hits++; },
    };
    setActiveTextView(actions);
    expect(activeTextView()).toBe(actions);
    activeTextView()?.toggleWrap();
    activeTextView()?.gotoBookmark(1);
    expect(hits).toBe(2);
    setActiveTextView(null);
    expect(activeTextView()).toBeNull();
  });

  it('注册栈：多文本视图并存时卸载其一不影响另一个', () => {
    resetTextViewBusForTest();
    const make = (): TextViewActions => ({
      toggleWrap: () => undefined,
      toggleBookmark: () => undefined,
      gotoBookmark: () => undefined,
      clearBookmarks: () => undefined,
      setBookmarkAtCursor: () => undefined,
    });
    const a = make();
    const b = make();
    const disposeA = registerTextView(a);
    const disposeB = registerTextView(b);
    expect(activeTextView()).toBe(b);
    disposeB();
    expect(activeTextView()).toBe(a); // 后挂载卸载后回退到先挂载
    disposeA();
    expect(activeTextView()).toBeNull();
  });
});
