import { describe, expect, it } from 'vitest';
import {
  applySidePatches,
  fullSideReplayOp,
  invertSidePatches,
  opsFromPatches,
  patchSideFromDocEdit,
  patchesFromLineReplacement,
  type SidePatch,
} from './editPatch';
import type { AlignedRow } from './alignment';
import type { TextEditOp } from '../../protocol/types';

/** 构造对齐行元数据：l/r 为 0 基侧行号，undefined = 填充行 */
function row(
  l: number | undefined,
  r: number | undefined,
  status: AlignedRow['status'],
): AlignedRow {
  return { leftLine: l, rightLine: r, status, sectionId: 0 };
}

describe('editPatch：对齐文档编辑 → 单侧内容补丁（编辑重 diff 映射核心）', () => {
  const rowsEqual3 = [row(0, 0, 'equal'), row(1, 1, 'equal'), row(2, 2, 'equal')];

  it('真实行内输入：单 change 补丁', () => {
    const res = patchSideFromDocEdit(rowsEqual3, ['abc', 'x', 'y'], ['axbc', 'x', 'y'], 'left', [
      'abc',
      'x',
      'y',
    ]);
    expect(res).not.toBeNull();
    expect(res!.patches).toEqual([{ start: 0, removed: ['abc'], inserted: ['axbc'] }]);
    expect(res!.newLines).toEqual(['axbc', 'x', 'y']);
  });

  it('行尾回车拆行：新增一行（纯插入补丁）', () => {
    const res = patchSideFromDocEdit(
      rowsEqual3,
      ['abc', 'x', 'y'],
      ['abc', 'xyz', 'x', 'y'],
      'left',
      ['abc', 'x', 'y'],
    );
    expect(res!.patches).toEqual([{ start: 1, removed: [], inserted: ['xyz'] }]);
    expect(res!.newLines).toEqual(['abc', 'xyz', 'x', 'y']);
  });

  it('合并两行（删除换行）：两行变一行', () => {
    const res = patchSideFromDocEdit(rowsEqual3, ['abc', 'x', 'y'], ['abcx', 'y'], 'left', [
      'abc',
      'x',
      'y',
    ]);
    expect(res!.patches).toEqual([{ start: 0, removed: ['abc', 'x'], inserted: ['abcx'] }]);
    expect(res!.newLines).toEqual(['abcx', 'y']);
  });

  it('填充行输入文本 → 该侧新增行（insert section 场景）', () => {
    // 左侧填充行（右独有行的对位），用户在左窗格该行输入 "newL"
    const rows = [
      row(0, 0, 'equal'),
      row(undefined, 1, 'inserted'), // 左填充
      row(1, 2, 'equal'),
    ];
    const res = patchSideFromDocEdit(rows, ['a', '', 'b'], ['a', 'newL', 'b'], 'left', ['a', 'b']);
    expect(res!.patches).toEqual([{ start: 1, removed: [], inserted: ['newL'] }]);
    expect(res!.newLines).toEqual(['a', 'newL', 'b']);
  });

  it('填充行保持空 → 无补丁（null，跳过重 diff）', () => {
    const rows = [row(0, 0, 'equal'), row(undefined, 1, 'inserted')];
    const res = patchSideFromDocEdit(rows, ['a', ''], ['a', ''], 'left', ['a']);
    expect(res).toBeNull();
  });

  it('删除整行（含真实行）→ 删除补丁', () => {
    const res = patchSideFromDocEdit(rowsEqual3, ['a', 'x', 'b'], ['a', 'b'], 'left', [
      'a',
      'x',
      'b',
    ]);
    expect(res!.patches).toEqual([{ start: 1, removed: ['x'], inserted: [] }]);
    expect(res!.newLines).toEqual(['a', 'b']);
  });

  it('多 hunk 编辑：两处独立修改 → 两个补丁，start 坐标顺序应用成立', () => {
    const res = patchSideFromDocEdit(rowsEqual3, ['a0', 'a1', 'a2'], ['A0', 'a1', 'A2'], 'left', [
      'a0',
      'a1',
      'a2',
    ]);
    const patches = res!.patches;
    expect(patches).toHaveLength(2);
    expect(patches[0]).toEqual({ start: 0, removed: ['a0'], inserted: ['A0'] });
    // 第二个补丁的 start 已计入第一个补丁的净变化（等行数 → 不变）
    expect(patches[1]).toEqual({ start: 2, removed: ['a2'], inserted: ['A2'] });
    expect(applySidePatches(['a0', 'a1', 'a2'], patches)).toEqual(['A0', 'a1', 'A2']);
  });

  it('多 hunk 行数不等：start 坐标换算（前 hunk 净增）', () => {
    const rows = [row(0, 0, 'equal'), row(1, 1, 'equal'), row(2, 2, 'equal')];
    const res = patchSideFromDocEdit(
      rows,
      ['a0', 'a1', 'a2'],
      ['n0', 'n1', 'a1', 'A2'], // hunk1: 1→2 行（净增 1）；hunk2: 改 a2
      'left',
      ['a0', 'a1', 'a2'],
    );
    const patches = res!.patches;
    expect(patches[0].start).toBe(0);
    expect(patches[1].start).toBe(3); // 原 2 + 净增 1
    expect(applySidePatches(['a0', 'a1', 'a2'], patches)).toEqual(['n0', 'n1', 'a1', 'A2']);
  });

  it('撤销往返：invert → apply 回到原内容', () => {
    const res = patchSideFromDocEdit(
      rowsEqual3,
      ['a0', 'a1', 'a2'],
      ['X0', 'x1', 'y1', 'a2'],
      'left',
      ['a0', 'a1', 'a2'],
    );
    const undone = applySidePatches(res!.newLines, invertSidePatches(res!.patches));
    expect(undone).toEqual(['a0', 'a1', 'a2']);
  });

  it('右侧编辑同样成立（镜像）', () => {
    const rows = [row(0, 0, 'equal'), row(1, undefined, 'deleted')];
    const res = patchSideFromDocEdit(rows, ['a', '', 'b'], ['a', 'R', 'b'], 'right', ['a', 'b']);
    expect(res!.patches).toEqual([{ start: 1, removed: [], inserted: ['R'] }]);
    expect(res!.newLines).toEqual(['a', 'R', 'b']);
  });

  it('空文档编辑首行', () => {
    const res = patchSideFromDocEdit([], [], ['hello'], 'left', []);
    expect(res!.patches).toEqual([{ start: 0, removed: [], inserted: ['hello'] }]);
  });
});

describe('applySidePatches / invertSidePatches 基础语义', () => {
  it('顺序应用与逆序反演（撤销链路）', () => {
    const lines = ['1', '2', '3'];
    const patches: SidePatch[] = [
      { start: 0, removed: ['1'], inserted: ['one', 'one-a'] },
      { start: 3, removed: ['3'], inserted: ['three!'] },
    ];
    const applied = applySidePatches(lines, patches);
    expect(applied).toEqual(['one', 'one-a', '2', 'three!']);
    expect(applySidePatches(applied, invertSidePatches(patches))).toEqual(lines);
  });
});

describe('patchesFromLineReplacement：全部替换的复合补丁', () => {
  it('连续变化行归并为一个补丁块', () => {
    const oldL = ['a=1', 'keep', 'b=2', 'c=3'];
    const newL = ['1=a', 'keep', '2=b', '3=c'];
    const patches = patchesFromLineReplacement(oldL, newL);
    expect(patches).toEqual([
      { start: 0, removed: ['a=1'], inserted: ['1=a'] },
      { start: 2, removed: ['b=2', 'c=3'], inserted: ['2=b', '3=c'] },
    ]);
    expect(applySidePatches(oldL, patches!)).toEqual(newL);
  });

  it('无变化 → null', () => {
    expect(patchesFromLineReplacement(['x'], ['x'])).toBeNull();
    expect(patchesFromLineReplacement([], [])).toBeNull();
  });

  it('撤销往返', () => {
    const oldL = ['ab', 'cd', 'ab'];
    const newL = ['X', 'cd', 'X'];
    const patches = patchesFromLineReplacement(oldL, newL)!;
    expect(applySidePatches(newL, invertSidePatches(patches))).toEqual(oldL);
  });
});

/**
 * 按引擎 text/edit 语义把 op 序列应用到行数组（顺序应用、坐标为应用后坐标、
 * text 以 \n 连接、空串 = 纯删除、end 省略 = 到该侧末尾）。
 */
function applyOps(lines: string[], ops: TextEditOp[]): string[] {
  const out = [...lines];
  for (const op of ops) {
    const end = op.end ?? out.length;
    const parts = op.text === '' ? [] : op.text.split('\n');
    out.splice(op.start, end - op.start, ...parts);
  }
  return out;
}

describe('opsFromPatches：UI 补丁 → 引擎 text/edit op（R3-18 引擎权威重 diff）', () => {
  it('op 序列与 applySidePatches 结果逐行一致（单块/多块/增删改混合）', () => {
    const cases: Array<{ lines: string[]; patches: SidePatch[] }> = [
      { lines: ['abc', 'x', 'y'], patches: [{ start: 0, removed: ['abc'], inserted: ['axbc'] }] },
      {
        lines: ['one', '2', 'three'],
        patches: [{ start: 1, removed: ['2'], inserted: ['2a', '2b'] }],
      },
      { lines: ['a', 'b', 'c'], patches: [{ start: 1, removed: ['b'], inserted: [] }] },
      { lines: ['a', 'c'], patches: [{ start: 1, removed: [], inserted: ['b'] }] },
      {
        lines: ['a=1', 'keep', 'b=2', 'c=3'],
        patches: [
          { start: 0, removed: ['a=1'], inserted: ['1=a'] },
          { start: 2, removed: ['b=2', 'c=3'], inserted: ['2=b', '3=c'] },
        ],
      },
      { lines: ['a', 'b'], patches: [{ start: 2, removed: [], inserted: ['c', 'd'] }] },
    ];
    for (const { lines, patches } of cases) {
      const ops = opsFromPatches('left', patches);
      expect(applyOps(lines, ops)).toEqual(applySidePatches(lines, patches));
      expect(ops.every((o) => o.side === 'left')).toBe(true);
    }
  });

  it('纯删除 → text 为空串；多行插入 → \\n 连接', () => {
    expect(opsFromPatches('right', [{ start: 1, removed: ['b'], inserted: [] }])).toEqual([
      { side: 'right', start: 1, end: 2, text: '' },
    ]);
    expect(opsFromPatches('right', [{ start: 0, removed: [], inserted: ['p', 'q'] }])).toEqual([
      { side: 'right', start: 0, end: 0, text: 'p\nq' },
    ]);
  });

  it('空补丁列表 → 空 op 列表（调用方据此跳过 RPC）', () => {
    expect(opsFromPatches('left', [])).toEqual([]);
  });
});

describe('fullSideReplayOp：整侧重放（引擎态与 UI 态分叉后的收敛手段）', () => {
  it('end 省略 = 到末尾，text 为全部行', () => {
    expect(fullSideReplayOp('left', ['a', 'b', 'c'])).toEqual({
      side: 'left',
      start: 0,
      text: 'a\nb\nc',
    });
  });

  it('重放后行表与 UI 完全一致（含空侧）', () => {
    expect(applyOps(['stale', 'lines', 'here'], [fullSideReplayOp('right', ['a', 'b'])])).toEqual([
      'a',
      'b',
    ]);
    expect(applyOps(['x'], [fullSideReplayOp('left', [])])).toEqual([]);
    expect(applyOps([], [fullSideReplayOp('left', ['x'])])).toEqual(['x']);
  });
});
