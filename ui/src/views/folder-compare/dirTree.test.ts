import { describe, expect, it } from 'vitest';
import type { DirRow } from '../../protocol/types';
import { buildTreeIndex, expandAll, orderedFlatRows, orderedTreeRows, parentOf, rowDepth, toggleExpanded } from './dirTree';

function dirRow(id: string, kind: DirRow['kind'] = 'file', hidden = false): DirRow {
  const side = { size: 1, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 };
  return {
    id,
    name: id.split('/').pop() ?? id,
    kind,
    left: side,
    right: side,
    status: 'same',
    hasChildren: kind === 'dir',
    hidden,
    suppressed: false,
  };
}

describe('dirTree：树索引与可见行', () => {
  const rows = [
    dirRow('b.txt'),
    dirRow('a-dir', 'dir'),
    dirRow('a-dir/inner.txt'),
    dirRow('a-dir/sub', 'dir'),
    dirRow('a-dir/sub/deep.txt'),
    dirRow('c.txt', 'file', true), // hidden 行不出现在树中
  ];
  const index = buildTreeIndex(rows);

  it('parentOf 与 rowDepth（rel 路径段）', () => {
    expect(parentOf('a-dir/inner.txt')).toBe('a-dir');
    expect(parentOf('top.txt')).toBe('');
    expect(rowDepth('a-dir/sub/deep.txt')).toBe(2);
    expect(rowDepth('top.txt')).toBe(0);
  });

  it('树序 = DFS 序，逐层目录先、名称次；hidden 行剔除', () => {
    const visible = orderedTreeRows(index, new Set(['a-dir']));
    // 顶层：a-dir（目录先）> b.txt；a-dir 展开 -> sub（目录先）> inner.txt
    expect(visible.map((r) => r.id)).toEqual(['a-dir', 'a-dir/sub', 'a-dir/inner.txt', 'b.txt']);
    expect(visible.some((r) => r.id === 'c.txt')).toBe(false);
  });

  it('未展开的目录其子树不出现；折叠后回到顶层', () => {
    expect(orderedTreeRows(index, new Set()).map((r) => r.id)).toEqual(['a-dir', 'b.txt']);
    const deep = orderedTreeRows(index, new Set(['a-dir', 'a-dir/sub']));
    expect(deep.map((r) => r.id)).toEqual(['a-dir', 'a-dir/sub', 'a-dir/sub/deep.txt', 'a-dir/inner.txt', 'b.txt']);
  });

  it('toggleExpanded 不可变切换', () => {
    const s = toggleExpanded(new Set(['a']), 'b');
    expect(s.has('a')).toBe(true);
    expect(s.has('b')).toBe(true);
    const s2 = toggleExpanded(s, 'b');
    expect(s2.has('b')).toBe(false);
  });

  it('expandAll 汇集全部目录行', () => {
    const all = expandAll(rows);
    expect(all.has('a-dir')).toBe(true);
    expect(all.has('a-dir/sub')).toBe(true);
    expect(all.has('b.txt')).toBe(false);
  });

  it('平铺序：只含文件、名称排序、hidden 剔除', () => {
    const flat = orderedFlatRows(rows);
    // 按名称排序（b.txt < deep.txt < inner.txt；目录行不出现）
    expect(flat.map((r) => r.id)).toEqual(['b.txt', 'a-dir/sub/deep.txt', 'a-dir/inner.txt']);
  });
});
