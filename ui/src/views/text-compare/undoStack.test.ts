import { describe, expect, it } from 'vitest';
import {
  canRedo,
  canUndo,
  emptyUndoState,
  popRedo,
  popUndo,
  pushUndo,
  redoPatches,
  tryCoalesce,
  undoPatches,
  type UndoEntry,
  type UndoState,
} from './undoStack';
import { applySidePatches } from './editPatch';

const entry = (
  side: 'left' | 'right',
  start: number,
  removed: string[],
  inserted: string[],
  at?: number,
): UndoEntry => ({
  side,
  patches: [{ start, removed, inserted }],
  lastAt: at,
});

/** 驱动：模拟"编辑→撤销→重做"全链路（行内容真实应用补丁） */
function drive(initial: string[], entries: UndoEntry[]) {
  let lines = initial;
  let st: UndoState = emptyUndoState();
  for (const e of entries) {
    lines = applySidePatches(lines, e.patches);
    st = pushUndo(st, e);
  }
  return {
    get lines() {
      return lines;
    },
    get state() {
      return st;
    },
    undo() {
      const popped = popUndo(st);
      if (!popped) return false;
      st = popped.state;
      lines = applySidePatches(lines, undoPatches(popped.entry));
      return true;
    },
    redo() {
      const popped = popRedo(st);
      if (!popped) return false;
      st = popped.state;
      lines = applySidePatches(lines, redoPatches(popped.entry));
      return true;
    },
  };
}

describe('undoStack：会话级撤销/重做（UNDO-001）', () => {
  it('LIFO 撤销：后编辑先撤销，行内容逐级还原', () => {
    const d = drive(
      ['a', 'b', 'c'],
      [
        entry('left', 0, ['a'], ['A']),
        entry('left', 3, [], ['X', 'Y']), // 尾部插入两行
        entry('left', 1, ['b'], ['B!']),
      ],
    );
    expect(d.lines).toEqual(['A', 'B!', 'c', 'X', 'Y']);
    expect(d.undo()).toBe(true);
    expect(d.lines).toEqual(['A', 'b', 'c', 'X', 'Y']);
    expect(d.undo()).toBe(true);
    expect(d.lines).toEqual(['A', 'b', 'c']);
    expect(d.undo()).toBe(true);
    expect(d.lines).toEqual(['a', 'b', 'c']);
    expect(d.undo()).toBe(false);
  });

  it('重做：撤销后按原序重放，内容与最终态一致', () => {
    const d = drive(['a'], [entry('left', 0, ['a'], ['A1']), entry('left', 1, [], ['Z'])]);
    expect(d.lines).toEqual(['A1', 'Z']);
    d.undo();
    d.undo();
    expect(d.lines).toEqual(['a']);
    d.redo();
    d.redo();
    expect(d.lines).toEqual(['A1', 'Z']);
    expect(d.redo()).toBe(false);
  });

  it('新编辑清除重做链', () => {
    const d = drive(['a'], [entry('left', 0, ['a'], ['A'])]);
    d.undo();
    expect(canRedo(d.state)).toBe(true);
    // 直接再编辑（不经撤销栈弹出）：新条目压入应清除重做链
    const edited = applySidePatches(d.lines, [{ start: 1, removed: [], inserted: ['w'] }]);
    expect(edited).toEqual(['a', 'w']);
    const st2 = pushUndo(d.state, entry('left', 1, [], ['w'], 0));
    expect(canRedo(st2)).toBe(false);
    expect(canUndo(st2)).toBe(true);
  });

  it('撤销上限：超过 UNDO_LIMIT 丢最旧', () => {
    let st = emptyUndoState();
    for (let i = 0; i < 210; i++) st = pushUndo(st, entry('left', 0, [], [`x${i}`]));
    expect(st.undo.length).toBe(200);
    expect(st.undo[0].patches[0].inserted[0]).toBe('x10');
  });

  it('tryCoalesce：同侧纯追加合并为一条（打字不逐键膨胀）', () => {
    let st = emptyUndoState();
    st = pushUndo(st, entry('left', 0, [], ['h'], 1000));
    st = tryCoalesce(st, entry('left', 1, [], ['e'], 1300), 1300);
    st = tryCoalesce(st, entry('left', 2, [], ['y'], 1600), 1600);
    expect(st.undo).toHaveLength(1);
    expect(st.undo[0].patches[0].inserted).toEqual(['h', 'e', 'y']);
    // 不同侧 / 非追加位置 / 超时 → 不合并
    st = tryCoalesce(st, entry('right', 0, [], ['x'], 1700), 1700);
    expect(st.undo).toHaveLength(2);
    st = tryCoalesce(st, entry('left', 99, [], ['q'], 1800), 1800);
    expect(st.undo).toHaveLength(3);
    st = tryCoalesce(st, entry('left', 3, [], ['!'], 99999), 99999);
    expect(st.undo).toHaveLength(4);
  });

  it('tryCoalesce：已有行内连续输入合并为 原→终 一条', () => {
    let st = emptyUndoState();
    // 行 "ab" → "abc"（补丁 removed=['ab'], inserted=['abc']）
    st = pushUndo(st, entry('left', 5, ['ab'], ['abc'], 1000));
    // 续输入 "abcd"
    st = tryCoalesce(st, entry('left', 5, ['abc'], ['abcd'], 1300), 1300);
    expect(st.undo).toHaveLength(1);
    expect(st.undo[0].patches[0]).toEqual({ start: 5, removed: ['ab'], inserted: ['abcd'] });
    // 链断（removed 与上次结果不接）→ 新条目
    st = tryCoalesce(st, entry('left', 5, ['zzz'], ['zzz!'], 1500), 1500);
    expect(st.undo).toHaveLength(2);
    // 合并条目在栈底：撤销它 = 一次回到原行内容
    expect(st.undo[0].patches[0]).toEqual({ start: 5, removed: ['ab'], inserted: ['abcd'] });
    expect(st.undo[1].patches[0]).toEqual({ start: 5, removed: ['zzz'], inserted: ['zzz!'] });
    const popped = popUndo(st)!;
    expect(popped.entry.patches[0].inserted).toEqual(['zzz!']);
  });

  it('跨侧撤销：左右窗格条目同栈交替撤销（会话级语义）', () => {
    // 注：drive 将两侧补丁应用到同一数组以验证栈的交错次序；真实视图按侧分别应用
    const d = drive(['a'], [entry('left', 0, ['a'], ['AL']), entry('right', 0, [], ['R1'])]);
    expect(d.lines).toEqual(['R1', 'AL']);
    d.undo();
    d.undo();
    expect(d.lines).toEqual(['a']);
    d.redo();
    d.redo();
    expect(d.lines).toEqual(['R1', 'AL']);
  });
});
