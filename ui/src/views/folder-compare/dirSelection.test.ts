import { describe, expect, it } from 'vitest';
import type { DirRow } from '../../protocol/types';
import {
  clearMarks,
  clearSelection,
  emptyMarks,
  emptySelection,
  invertSelection,
  isMarked,
  isOlder,
  onRowClick,
  runEditSelectCommand,
  selectAll,
  selectWhere,
  selectedIdsInOrder,
  setMarks,
  toggleMark,
} from './dirSelection';

/** FR-DIR-050 选择模型状态机 */
describe('dirSelection', () => {
  const visible = ['a.txt', 'b.txt', 'c.txt', 'sub/d.txt', 'sub/e.txt'];

  it('普通单击 = 单选并重置锚点', () => {
    const s = onRowClick(emptySelection, 'b.txt', { ctrl: false, shift: false }, visible);
    expect([...s.selected]).toEqual(['b.txt']);
    expect(s.anchor).toBe('b.txt');
  });

  it('Ctrl+单击 累加选择', () => {
    const s1 = onRowClick(emptySelection, 'a.txt', { ctrl: false, shift: false }, visible);
    const s2 = onRowClick(s1, 'c.txt', { ctrl: true, shift: false }, visible);
    expect(s2.selected.has('a.txt')).toBe(true);
    expect(s2.selected.has('c.txt')).toBe(true);
    expect(s2.selected.size).toBe(2);
  });

  it('Ctrl+单击 已选项 = 反选移除', () => {
    const s1 = onRowClick(emptySelection, 'a.txt', { ctrl: false, shift: false }, visible);
    const s2 = onRowClick(s1, 'a.txt', { ctrl: true, shift: false }, visible);
    expect(s2.selected.size).toBe(0);
  });

  it('Shift+单击 = 锚点到当前行的范围连选', () => {
    const s1 = onRowClick(emptySelection, 'a.txt', { ctrl: false, shift: false }, visible);
    const s2 = onRowClick(s1, 'c.txt', { ctrl: false, shift: true }, visible);
    expect([...s2.selected]).toEqual(['a.txt', 'b.txt', 'c.txt']);
    expect(s2.anchor).toBe('a.txt');
  });

  it('Shift+单击 反向区间（锚点在当前行之后）', () => {
    const s1 = onRowClick(emptySelection, 'c.txt', { ctrl: false, shift: false }, visible);
    const s2 = onRowClick(s1, 'a.txt', { ctrl: false, shift: true }, visible);
    expect([...s2.selected]).toEqual(['a.txt', 'b.txt', 'c.txt']);
  });

  it('锚点失效（不可见）时 Shift 退化为单选', () => {
    const s1 = { anchor: 'ghost', selected: new Set(['ghost']) };
    const s2 = onRowClick(s1, 'b.txt', { ctrl: false, shift: true }, visible);
    expect([...s2.selected]).toEqual(['b.txt']);
  });

  it('selectAll / invertSelection / clearSelection', () => {
    const all = selectAll(visible);
    expect(all.selected.size).toBe(visible.length);
    const inv = invertSelection(all, visible);
    expect(inv.selected.size).toBe(0);
    const inv2 = invertSelection(
      onRowClick(emptySelection, 'b.txt', { ctrl: false, shift: false }, visible),
      visible,
    );
    expect(inv2.selected.size).toBe(visible.length - 1);
    expect(inv2.selected.has('b.txt')).toBe(false);
    expect(clearSelection().selected.size).toBe(0);
  });

  it('selectWhere 按谓词选择（孤立/较新命令数据面）', () => {
    const rows = [
      { id: 'a', visible: true },
      { id: 'b', visible: true },
      { id: 'h', visible: false }, // 被过滤隐藏行不参与
    ];
    const s = selectWhere(rows, (r) => r.id === 'a' || r.id === 'h');
    expect([...s.selected]).toEqual(['a']);
  });

  it('selectedIdsInOrder 保持可见行序', () => {
    const s = { anchor: null, selected: new Set(['c.txt', 'a.txt']) };
    expect(selectedIdsInOrder(s, visible)).toEqual(['a.txt', 'c.txt']);
  });
});

/** P6b：标记集（空格标记）纯函数 */
describe('dirSelection marks', () => {
  it('toggleMark 切换且不可变（原集合不动）', () => {
    const m0 = emptyMarks;
    const m1 = toggleMark(m0, 'a.txt');
    expect(isMarked(m1, 'a.txt')).toBe(true);
    expect(isMarked(m0, 'a.txt')).toBe(false);
    const m2 = toggleMark(m1, 'a.txt');
    expect(isMarked(m2, 'a.txt')).toBe(false);
    expect(m1.size).toBe(1);
  });

  it('setMarks 批量打标/取消', () => {
    const m1 = setMarks(emptyMarks, ['a', 'b', 'c'], true);
    expect(m1.size).toBe(3);
    const m2 = setMarks(m1, ['b', 'c'], false);
    expect([...m2]).toEqual(['a']);
  });

  it('clearMarks 清空', () => {
    const m1 = setMarks(emptyMarks, ['x', 'y'], true);
    expect(clearMarks().size).toBe(0);
    expect(m1.size).toBe(2); // 原集合不动
  });
});

/** P6b：编辑菜单命令选择器（FR-DIR-050/§3.1.6） */
describe('dirSelection edit commands', () => {
  function mkRow(
    id: string,
    status: DirRow['status'],
    lMtime: number,
    rMtime: number,
    kind: DirRow['kind'] = 'file',
  ): DirRow {
    const side = (mtime: number | null) =>
      mtime === null ? undefined : { size: 1, mtimeMs: mtime, unixMode: 0, uid: 0, gid: 0 };
    return {
      id,
      name: id.split('/').pop() ?? id,
      kind,
      left: side(lMtime),
      right: side(rMtime),
      status,
      hasChildren: kind === 'dir',
      hidden: false,
      suppressed: false,
      leftPath: lMtime === null ? undefined : id,
      rightPath: rMtime === null ? undefined : id,
    };
  }

  const rows = [
    { ...mkRow('newerL', 'quickDiff', 200, 100), visible: true },
    { ...mkRow('newerR', 'quickDiff', 100, 200), visible: true },
    { ...mkRow('olderPair', 'quickDiff', 50, 90), visible: true }, // 两侧均判：左更旧、右更新
    { ...mkRow('orphanL', 'leftOnly', 10, 0), visible: true },
    { ...mkRow('orphanR', 'rightOnly', 0, 10), visible: true },
    { ...mkRow('sameRow', 'same', 10, 10), visible: true },
    { ...mkRow('dirRow', 'dirDiff', 10, 10, 'dir'), visible: true },
    { ...mkRow('hiddenRow', 'leftOnly', 10, 0), visible: false }, // 被显示过滤隐藏
  ];
  const visibleIds = rows.filter((r) => r.visible).map((r) => r.id);

  it('选择较新的（左/右/两侧）', () => {
    expect([
      ...runEditSelectCommand('selectNewerLeft', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['newerL']);
    expect([
      ...runEditSelectCommand('selectNewerRight', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['newerR', 'olderPair']);
    expect([
      ...runEditSelectCommand('selectNewerBoth', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['newerL', 'newerR', 'olderPair']);
  });

  it('选择较旧的（左/右/两侧；与较新互补）', () => {
    expect(isOlder(rows[0], 'left')).toBe(false);
    expect([
      ...runEditSelectCommand('selectOlderLeft', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['newerR', 'olderPair']);
    expect([
      ...runEditSelectCommand('selectOlderRight', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['newerL']);
    expect([
      ...runEditSelectCommand('selectOlderBoth', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['newerL', 'newerR', 'olderPair']);
  });

  it('选择孤立项目（左/右/两侧）', () => {
    expect([
      ...runEditSelectCommand('selectOrphanLeft', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['orphanL']);
    expect([
      ...runEditSelectCommand('selectOrphanRight', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['orphanR']);
    expect([
      ...runEditSelectCommand('selectOrphanBoth', emptySelection, rows, visibleIds).selected,
    ]).toEqual(['orphanL', 'orphanR']);
  });

  it('选择所有文件（目录排除、隐藏行排除）', () => {
    const s = runEditSelectCommand('selectFiles', emptySelection, rows, visibleIds);
    expect(s.selected.has('dirRow')).toBe(false);
    expect(s.selected.has('hiddenRow')).toBe(false);
    expect(s.selected.size).toBe(6);
  });

  it('全选/反选以当前可见行为界', () => {
    const all = runEditSelectCommand('selectAll', emptySelection, rows, visibleIds);
    expect(all.selected.size).toBe(visibleIds.length);
    const inv = runEditSelectCommand('invert', all, rows, visibleIds);
    expect(inv.selected.size).toBe(0);
    const one = runEditSelectCommand(
      'invert',
      { anchor: null, selected: new Set(['newerL']) },
      rows,
      visibleIds,
    );
    expect(one.selected.size).toBe(visibleIds.length - 1);
    expect(one.selected.has('newerL')).toBe(false);
  });

  it('isOlder 谓词仅在 quickDiff 且对侧严格更新时为真', () => {
    expect(isOlder(mkRow('a', 'quickDiff', 1, 2), 'left')).toBe(true);
    expect(isOlder(mkRow('a', 'quickDiff', 2, 1), 'left')).toBe(false);
    expect(isOlder(mkRow('a', 'same', 1, 2), 'left')).toBe(false);
    expect(isOlder(mkRow('a', 'quickDiff', 1, 1), 'left')).toBe(false);
  });
});
