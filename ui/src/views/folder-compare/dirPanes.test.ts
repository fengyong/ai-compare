import { describe, expect, it } from 'vitest';
import type { DirRow } from '../../protocol/types';
import { emptySelection, type SelectionState } from './dirSelection';
import {
  gutterCopyDirs,
  nextSortSpec,
  paneColumns,
  scrollSyncPlan,
  sidePresent,
  withPaneSelection,
  type PaneSelections,
} from './dirPanes';

function row(id: string, left: boolean, right: boolean): DirRow {
  const side = { size: 10, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 };
  return {
    id,
    name: id,
    kind: 'file',
    left: left ? side : null,
    right: right ? side : null,
    status: left && right ? 'same' : left ? 'leftOnly' : 'rightOnly',
    hasChildren: false,
    hidden: false,
    suppressed: false,
  };
}

describe('dirPanes：双窗格列定义', () => {
  it('两侧各自 名称/大小/已修改 三列，排序键落到本侧', () => {
    const l = paneColumns('left');
    const r = paneColumns('right');
    expect(l.map((c) => c.key)).toEqual(['name', 'leftSize', 'leftMtime']);
    expect(r.map((c) => c.key)).toEqual(['name', 'rightSize', 'rightMtime']);
    expect(l.map((c) => c.label)).toEqual(['名称', '大小', '已修改']);
    expect(r.map((c) => c.label)).toEqual(['名称', '大小', '已修改']);
    // 名称列两侧同键：点任一侧名称表头都排同一份对齐行序
    expect(l[0].key).toBe(r[0].key);
    expect(l[1].align).toBe('r');
    expect(l[0].align).toBe('');
  });
});

describe('dirPanes：缺侧留白与 gutter 箭头', () => {
  it('孤立项只在存在侧渲染，缺侧留白', () => {
    const lo = row('only-left.txt', true, false);
    const ro = row('only-right.txt', false, true);
    expect(sidePresent(lo, 'left')).toBe(true);
    expect(sidePresent(lo, 'right')).toBe(false);
    expect(sidePresent(ro, 'left')).toBe(false);
    expect(sidePresent(ro, 'right')).toBe(true);
  });

  it('复制箭头方向 = 源侧存在（左孤立只能 →右，右孤立只能 ←左）', () => {
    expect(gutterCopyDirs(row('both.txt', true, true))).toEqual({ toLeft: true, toRight: true });
    expect(gutterCopyDirs(row('lo.txt', true, false))).toEqual({ toLeft: false, toRight: true });
    expect(gutterCopyDirs(row('ro.txt', false, true))).toEqual({ toLeft: true, toRight: false });
  });
});

describe('dirPanes：表头排序循环（FR-DIR-042）', () => {
  it('同键三击：升序 → 降序 → 默认', () => {
    const a = nextSortSpec(null, 'name');
    expect(a).toEqual({ key: 'name', dir: 1 });
    const b = nextSortSpec(a, 'name');
    expect(b).toEqual({ key: 'name', dir: -1 });
    expect(nextSortSpec(b, 'name')).toBeNull();
  });

  it('换键从升序重新开始（不继承上一键的方向）', () => {
    const desc = nextSortSpec(nextSortSpec(null, 'leftSize'), 'leftSize');
    expect(desc).toEqual({ key: 'leftSize', dir: -1 });
    expect(nextSortSpec(desc, 'rightMtime')).toEqual({ key: 'rightMtime', dir: 1 });
  });
});

describe('dirPanes：两侧选择独立（FR-DIR-003/050）', () => {
  const sel = (ids: string[]): SelectionState => ({ anchor: ids[0] ?? null, selected: new Set(ids) });

  it('写入一侧不影响另一侧，且不改动原对象', () => {
    const base: PaneSelections = { left: sel(['a.txt']), right: sel(['b.txt', 'c.txt']) };
    const next = withPaneSelection(base, 'left', sel(['z.txt']));
    expect([...next.left.selected]).toEqual(['z.txt']);
    expect([...next.right.selected]).toEqual(['b.txt', 'c.txt']);
    expect([...base.left.selected]).toEqual(['a.txt']);
    expect(next).not.toBe(base);
  });

  it('清空一侧保留另一侧', () => {
    const base: PaneSelections = { left: sel(['a.txt']), right: sel(['b.txt']) };
    const next = withPaneSelection(base, 'right', emptySelection);
    expect(next.right.selected.size).toBe(0);
    expect(next.left.selected.size).toBe(1);
  });
});

describe('dirPanes：滚动同步不回环', () => {
  it('跳过源自身，只给未对齐的列下发目标值', () => {
    const plan = scrollSyncPlan('left', 120, [
      { id: 'left', top: 120 },
      { id: 'gutter', top: 0 },
      { id: 'right', top: 120 },
    ]);
    expect(plan).toEqual([{ id: 'gutter', top: 120 }]);
  });

  it('全部已对齐时下发空计划（赋值触发的二次 scroll 事件就此终止）', () => {
    const plan = scrollSyncPlan('right', 80, [
      { id: 'left', top: 80 },
      { id: 'gutter', top: 80 },
      { id: 'right', top: 80 },
    ]);
    expect(plan).toEqual([]);
  });

  it('横向同步同形：窗格体 scrollLeft 带动本侧表头', () => {
    expect(scrollSyncPlan('body', 40, [{ id: 'head', top: 0 }])).toEqual([{ id: 'head', top: 40 }]);
    expect(scrollSyncPlan('body', 40, [{ id: 'head', top: 40 }])).toEqual([]);
  });
});