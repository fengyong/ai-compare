import { describe, expect, it } from 'vitest';
import type { DirRow, PlannedOperation } from '../../protocol/types';
import {
  buildOpsForCommand,
  hasIrreversible,
  hasSelectedDescendant,
  opsFromPlan,
  otherSide,
  plannedToOp,
  summarizeOps,
  summarizePlan,
  SYNC_PRESETS,
} from './dirOps';

function row(partial: Partial<DirRow> & { id: string }): DirRow {
  return {
    id: partial.id,
    name: partial.name ?? partial.id.split('/').pop()!,
    kind: partial.kind ?? 'file',
    left: partial.left !== undefined ? partial.left : { size: 1, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 },
    right: partial.right !== undefined ? partial.right : { size: 1, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 },
    status: partial.status ?? 'same',
    hasChildren: partial.hasChildren ?? false,
    hidden: partial.hidden ?? false,
    suppressed: partial.suppressed ?? false,
  };
}

const noSide = null;

describe('dirOps：批次构造（FR-DIR-060 文件夹语义）', () => {
  const rows = [
    row({ id: 'a.txt', status: 'contentDiff' }),
    row({ id: 'lonly.txt', status: 'leftOnly', right: noSide }),
    row({ id: 'ronly.txt', status: 'rightOnly', left: noSide }),
    row({ id: 'tree', kind: 'dir', status: 'dirDiff', hasChildren: true }),
    row({ id: 'tree/inner.txt', status: 'contentDiff' }),
  ];

  it('复制到右侧：源侧存在才生成 op；左孤立不可复制到左', () => {
    const ops = buildOpsForCommand('copyToRight', rows, new Set(['a.txt', 'ronly.txt']));
    expect(ops.map((o) => o.path)).toEqual(['a.txt']);
    const back = buildOpsForCommand('copyToLeft', rows, new Set(['a.txt', 'lonly.txt']));
    expect(back.map((o) => o.path)).toEqual(['a.txt']);
  });

  it('move 语义：copy + 删源（kind=move）', () => {
    const ops = buildOpsForCommand('moveToRight', rows, new Set(['a.txt']));
    expect(ops).toEqual([{ kind: 'move', path: 'a.txt', side: 'left', targetSide: 'right' }]);
  });

  it('目录整树一个 op；子项同被选中时忽略目录项（FR-DIR-060）', () => {
    const onlyDir = buildOpsForCommand('copyToRight', rows, new Set(['tree']));
    expect(onlyDir.map((o) => o.path)).toEqual(['tree']);
    const dirAndChild = buildOpsForCommand('copyToRight', rows, new Set(['tree', 'tree/inner.txt']));
    expect(dirAndChild.map((o) => o.path)).toEqual(['tree/inner.txt']);
  });

  it('delete = 双侧各一个 op（孤立项只删存在侧）', () => {
    const ops = buildOpsForCommand('delete', rows, new Set(['a.txt', 'lonly.txt']));
    expect(ops).toEqual([
      { kind: 'delete', path: 'a.txt', side: 'left' },
      { kind: 'delete', path: 'a.txt', side: 'right' },
      { kind: 'delete', path: 'lonly.txt', side: 'left' },
    ]);
    expect(hasIrreversible(ops)).toBe(true);
  });

  it('rename 单选 + 新名；touch 双侧对时', () => {
    const ren = buildOpsForCommand('rename', rows, new Set(['a.txt']), { newName: 'b.txt' });
    expect(ren).toEqual([{ kind: 'rename', path: 'a.txt', side: 'left', newName: 'b.txt' }]);
    const multi = buildOpsForCommand('rename', rows, new Set(['a.txt', 'lonly.txt']), { newName: 'b.txt' });
    expect(multi).toEqual([]);
    const touch = buildOpsForCommand('touch', rows, new Set(['lonly.txt']));
    expect(touch).toEqual([{ kind: 'touch', path: 'lonly.txt', side: 'left', mtimeMs: 0 }]);
  });

  it('汇总与不可逆判定', () => {
    const ops = [
      ...buildOpsForCommand('copyToRight', rows, new Set(['a.txt'])),
      ...buildOpsForCommand('delete', rows, new Set(['lonly.txt'])),
    ];
    const summary = summarizeOps(ops);
    expect(summary).toMatchObject({ copies: 1, deletes: 1, total: 2 });
    expect(hasIrreversible(ops)).toBe(true);
    expect(hasIrreversible(buildOpsForCommand('copyToRight', rows, new Set(['a.txt'])))).toBe(false);
  });

  it('hasSelectedDescendant 判定子项覆盖', () => {
    expect(hasSelectedDescendant('tree', new Set(['tree/inner.txt']))).toBe(true);
    expect(hasSelectedDescendant('tree', new Set(['tree', 'other.txt']))).toBe(false);
    expect(hasSelectedDescendant('tre', new Set(['tree/inner.txt']))).toBe(false);
  });
});

describe('dirOps：引擎计划 -> 执行操作（同步预设链路）', () => {
  it('plannedToOp 全枚举映射', () => {
    const plan: PlannedOperation[] = [
      { op: 'copyLeftToRight', path: 'a', reason: '' },
      { op: 'copyRightToLeft', path: 'b', reason: '' },
      { op: 'deleteLeft', path: 'c', reason: '' },
      { op: 'deleteRight', path: 'd', reason: '' },
      { op: 'touchLeft', path: 'e', reason: '' },
      { op: 'touchRight', path: 'f', reason: '' },
    ];
    const ops = opsFromPlan(plan);
    expect(ops[0]).toEqual({ kind: 'copy', path: 'a', side: 'left', targetSide: 'right' });
    expect(ops[1]).toEqual({ kind: 'copy', path: 'b', side: 'right', targetSide: 'left' });
    expect(ops[2]).toEqual({ kind: 'delete', path: 'c', side: 'left' });
    expect(ops[3]).toEqual({ kind: 'delete', path: 'd', side: 'right' });
    expect(ops[4]).toEqual({ kind: 'touch', path: 'e', side: 'left', mtimeMs: 0 });
    expect(ops[5]).toEqual({ kind: 'touch', path: 'f', side: 'right', mtimeMs: 0 });
    expect(plannedToOp(plan[0]).kind).toBe('copy');
  });

  it('summarizePlan 按方向计数', () => {
    const plan: PlannedOperation[] = [
      { op: 'copyLeftToRight', path: 'a', reason: '' },
      { op: 'deleteRight', path: 'b', reason: '' },
      { op: 'copyLeftToRight', path: 'c', reason: '' },
    ];
    const s = summarizePlan(plan);
    expect(s.copyLeftToRight).toBe(2);
    expect(s.deleteRight).toBe(1);
    expect(s.total).toBe(3);
  });

  it('SYNC_PRESETS 完整（FR-DIR-067 五预设）', () => {
    expect(SYNC_PRESETS).toHaveLength(5);
    expect(SYNC_PRESETS).toContain('mirrorLeftToRight');
    expect(SYNC_PRESETS).toContain('updateBoth');
    expect(otherSide('left')).toBe('right');
  });
});
