import { describe, expect, it } from 'vitest';
import type { DirRow } from '../../protocol/types';
import { currentDiffIndex, orderedDiffFileRows, pickDiffTarget } from './diffFileNav';

/** 子会话差异文件遍历（FR-DIR-071）：遍历序 / 当前对定位 / 回绕 */
const row = (id: string, over: Partial<DirRow> = {}): DirRow => ({
  id,
  name: id.split('/').pop() ?? id,
  kind: 'file',
  left: { size: 1, mtimeMs: 0, unixMode: 0, uid: 0, gid: 0 },
  right: { size: 1, mtimeMs: 0, unixMode: 0, uid: 0, gid: 0 },
  status: 'contentDiff',
  hasChildren: false,
  hidden: false,
  suppressed: false,
  leftPath: id,
  rightPath: id,
  ...over,
});

const roots = { left: '/L', right: '/R' };
const ids = (rows: readonly DirRow[]): string[] => rows.map((r) => r.id);

describe('orderedDiffFileRows', () => {
  it('只保留两侧都在的差异文件：目录/孤立项/相同/未知都排除', () => {
    const rows = [
      row('pair.txt'),
      row('quick.txt', { status: 'quickDiff' }),
      row('similar.txt', { status: 'similar' }),
      row('same.txt', { status: 'same' }),
      row('unknown.txt', { status: 'unknown' }),
      row('dir', { kind: 'dir', status: 'dirDiff' }),
      row('left-only.txt', { status: 'leftOnly', right: undefined, rightPath: undefined }),
      row('right-only.txt', { status: 'rightOnly', left: undefined, leftPath: undefined }),
    ];
    expect(ids(orderedDiffFileRows(rows))).toEqual(['pair.txt', 'quick.txt', 'similar.txt']);
  });

  it('按整棵树名称序遍历（目录先、逐层 DFS），不受展开状态限制', () => {
    const rows = [
      row('b.txt'),
      row('sub', { kind: 'dir', status: 'dirDiff' }),
      row('sub/z.txt'),
      row('a.txt'),
      row('sub/a.txt'),
    ];
    expect(ids(orderedDiffFileRows(rows))).toEqual(['sub/a.txt', 'sub/z.txt', 'a.txt', 'b.txt']);
  });

  it('hidden/suppressed 只是显示过滤呈现标记，差异文件仍参与遍历', () => {
    const rows = [
      row('a.txt'),
      row('hidden.txt', { hidden: true }),
      row('sup.txt', { suppressed: true }),
    ];
    expect(ids(orderedDiffFileRows(rows))).toEqual(['a.txt', 'hidden.txt', 'sup.txt']);
  });

  it('父视图"仅差异"隐藏 dirSame 目录时，子树差异文件不得一起消失（浏览器实测回归）', () => {
    const rows = [
      row('main.c'),
      row('src', { kind: 'dir', status: 'dirSame', hidden: true }),
      row('src/app6.xml'),
      row('src/deep', { kind: 'dir', status: 'dirSame', hidden: true }),
      row('src/deep/sub_1.rs', { status: 'quickDiff' }),
      row('docs', { kind: 'dir', status: 'dirSame', hidden: true }),
      row('docs/main6.ts'),
    ];
    expect(ids(orderedDiffFileRows(rows))).toEqual([
      'docs/main6.ts',
      'src/deep/sub_1.rs',
      'src/app6.xml',
      'main.c',
    ]);
  });

  it('空会话返回空列表', () => {
    expect(orderedDiffFileRows([])).toEqual([]);
  });
});

describe('currentDiffIndex', () => {
  it('平铺模式按真实路径定位：id 是显示名，不能当地址（R3-03）', () => {
    const rows = [
      row('a.txt', { leftPath: 'one/a.txt', rightPath: 'two/a.txt' }),
      row('a.txt#1', { leftPath: 'three/a.txt', rightPath: 'four/a.txt' }),
    ];
    const ordered = orderedDiffFileRows(rows);
    expect(
      currentDiffIndex(ordered, { leftPath: '/L/three/a.txt', rightPath: '/R/four/a.txt' }, roots),
    ).toBe(1);
  });

  it('手动对齐：左右不同名也能定位', () => {
    const ordered = orderedDiffFileRows([
      row('x.txt', { leftPath: 'src/x.txt', rightPath: 'dst/y.txt' }),
    ]);
    expect(
      currentDiffIndex(ordered, { leftPath: '/L/src/x.txt', rightPath: '/R/dst/y.txt' }, roots),
    ).toBe(0);
  });

  it('只有一侧路径相同不算命中', () => {
    const ordered = orderedDiffFileRows([row('a.txt')]);
    expect(
      currentDiffIndex(ordered, { leftPath: '/L/a.txt', rightPath: '/R/other.txt' }, roots),
    ).toBe(-1);
  });

  it('当前对不在差异列表时返回 -1（保存后已判相同）', () => {
    const ordered = orderedDiffFileRows([row('a.txt', { status: 'same' }), row('b.txt')]);
    expect(currentDiffIndex(ordered, { leftPath: '/L/a.txt', rightPath: '/R/a.txt' }, roots)).toBe(
      -1,
    );
  });

  it('根目录带尾分隔符不影响定位', () => {
    const ordered = orderedDiffFileRows([row('a.txt')]);
    expect(
      currentDiffIndex(
        ordered,
        { leftPath: '/L/a.txt', rightPath: '/R/a.txt' },
        { left: '/L/', right: '/R/' },
      ),
    ).toBe(0);
  });
});

describe('pickDiffTarget', () => {
  const ordered = orderedDiffFileRows([row('a.txt'), row('b.txt'), row('c.txt')]);

  it('next/prev 逐个前进后退', () => {
    expect(pickDiffTarget(ordered, 0, 'next')?.id).toBe('b.txt');
    expect(pickDiffTarget(ordered, 2, 'prev')?.id).toBe('b.txt');
  });

  it('两端回绕（BC 默认回绕到第一个差异）', () => {
    expect(pickDiffTarget(ordered, 2, 'next')?.id).toBe('a.txt');
    expect(pickDiffTarget(ordered, 0, 'prev')?.id).toBe('c.txt');
  });

  it('当前对不在列表时从首/尾进入', () => {
    expect(pickDiffTarget(ordered, -1, 'next')?.id).toBe('a.txt');
    expect(pickDiffTarget(ordered, -1, 'prev')?.id).toBe('c.txt');
  });

  it('只有一个差异对时 next/prev 都回到自身（调用方据此提示没有更多）', () => {
    const one = orderedDiffFileRows([row('only.txt')]);
    expect(pickDiffTarget(one, 0, 'next')?.id).toBe('only.txt');
    expect(pickDiffTarget(one, 0, 'prev')?.id).toBe('only.txt');
  });

  it('空列表返回 null', () => {
    expect(pickDiffTarget([], 0, 'next')).toBeNull();
    expect(pickDiffTarget([], -1, 'prev')).toBeNull();
  });
});
