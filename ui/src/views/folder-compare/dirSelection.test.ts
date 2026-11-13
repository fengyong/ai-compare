import { describe, expect, it } from 'vitest';
import {
  clearSelection,
  emptySelection,
  invertSelection,
  onRowClick,
  selectAll,
  selectWhere,
  selectedIdsInOrder,
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
    const inv2 = invertSelection(onRowClick(emptySelection, 'b.txt', { ctrl: false, shift: false }, visible), visible);
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
