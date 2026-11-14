import { describe, expect, it } from 'vitest';
import type { DirMergeRow, DirMergeDecision, DirMergeOpKind, DirMergePlannedOp } from '../../protocol/types';
import {
  applyDecisionToTree,
  defaultExpanded,
  effectiveDecision,
  exportMergeReportText,
  hasMergeChildren,
  mergeDecisionStats,
  mergeStateClass,
  orderedMergeRows,
  mergePreviewFromPlan,
  summarizeMergePlan,
  type DecisionMap,
} from './mergeRows';

let seq = 0;
/** 行构造（relPath/形态/分类/状态） */
function mrow(
  relPath: string,
  opts: Partial<Pick<DirMergeRow, 'kind' | 'category' | 'rowState' | 'parent' | 'conflictLeaf' | 'left' | 'right' | 'base'>> = {},
): DirMergeRow {
  seq++;
  const kind = opts.kind ?? 'file';
  return {
    relPath,
    name: relPath.split('/').pop() ?? relPath,
    kind,
    base: opts.base ?? null,
    left: opts.left ?? (kind === 'dir' ? { size: 0, mtimeMs: seq, attribute: null, unixMode: 0, uid: 0, gid: 0 } : null),
    right: opts.right ?? null,
    category: opts.category ?? 'same',
    rowState: opts.rowState ?? 'same',
    parent: opts.parent ?? (relPath.includes('/') ? relPath.slice(0, relPath.lastIndexOf('/')) : null),
    depth: relPath.split('/').length,
    conflictLeaf: opts.conflictLeaf ?? false,
  };
}

/** 混合树：目录 d 含冲突子 cf 与自动子 lc；顶层含左孤立 lo / 右改 rc / 一致 same */
function mixedRows(): DirMergeRow[] {
  return [
    mrow('d', { kind: 'dir', category: 'conflict', rowState: 'conflict' }),
    mrow('d/cf', { parent: 'd', category: 'conflict', rowState: 'conflict', conflictLeaf: true, left: { size: 1, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 }, right: { size: 2, mtimeMs: 2, attribute: null, unixMode: 0, uid: 0, gid: 0 } }),
    mrow('d/lc', { parent: 'd', category: 'leftChanged', rowState: 'auto' }),
    mrow('lo', { category: 'leftOnly', rowState: 'orphan', left: { size: 3, mtimeMs: 3, attribute: null, unixMode: 0, uid: 0, gid: 0 } }),
    mrow('rc', { category: 'rightChanged', rowState: 'auto' }),
    mrow('same', { category: 'same', rowState: 'same' }),
  ];
}

describe('folder-merge/mergeRows 树序与着色', () => {
  it('orderedMergeRows 按 DFS 序（父先于子）', () => {
    const ordered = orderedMergeRows(mixedRows(), defaultExpanded(mixedRows()));
    const paths = ordered.map((r) => r.relPath);
    expect(paths.indexOf('d')).toBeLessThan(paths.indexOf('d/cf'));
    expect(paths.indexOf('d/cf')).toBeLessThan(paths.indexOf('d/lc'));
    expect(paths).toContain('lo');
  });

  it('折叠目录隐藏子树', () => {
    const ordered = orderedMergeRows(mixedRows(), new Set());
    expect(ordered.map((r) => r.relPath)).toEqual(['d', 'lo', 'rc', 'same']);
  });

  it('行状态着色口径（绿/红/灰/暗）', () => {
    expect(mergeStateClass('auto')).toBe('merge-auto');
    expect(mergeStateClass('conflict')).toBe('merge-conflict');
    expect(mergeStateClass('orphan')).toBe('merge-orphan');
    expect(mergeStateClass('same')).toBe('merge-same');
  });

  it('hasMergeChildren/defaultExpanded', () => {
    const rows = mixedRows();
    expect(hasMergeChildren(rows, 'd')).toBe(true);
    expect(hasMergeChildren(rows, 'lo')).toBe(false);
    const expanded = defaultExpanded(rows);
    expect(expanded.has('d')).toBe(true);
    expect(expanded.has('lo')).toBe(false);
  });
});

describe('folder-merge/mergeRows 决策状态机（继承/展开/聚合）', () => {
  it('有效决策 = 自身显式 > 祖先 > auto', () => {
    const decisions: DecisionMap = { d: 'takeLeft' };
    expect(effectiveDecision('d/cf', decisions)).toBe('takeLeft');
    expect(effectiveDecision('lo', decisions)).toBe('auto');
    expect(effectiveDecision('d', decisions)).toBe('takeLeft');
  });

  it('最近祖先优先于更远祖先', () => {
    const rows = [...mixedRows(), mrow('d/sub', { kind: 'dir', parent: 'd', category: 'same', rowState: 'same' }), mrow('d/sub/x', { parent: 'd/sub' })];
    const decisions: DecisionMap = { d: 'takeLeft', 'd/sub': 'takeRight' };
    expect(effectiveDecision('d/sub/x', decisions)).toBe('takeRight');
    void rows;
  });

  it('目录决策递归展开到子树（覆盖既有决策）', () => {
    const rows = mixedRows();
    const before: DecisionMap = { 'd/cf': 'takeRight' };
    const next = applyDecisionToTree(rows, before, 'd', 'takeLeft');
    expect(next['d']).toBe('takeLeft');
    expect(next['d/cf']).toBe('takeLeft');
    expect(next['d/lc']).toBe('takeLeft');
    expect(next['lo']).toBeUndefined();
  });

  it('auto 决策清除显式项', () => {
    const rows = mixedRows();
    const withDecisions = applyDecisionToTree(rows, {}, 'd', 'skip');
    expect(withDecisions['d/cf']).toBe('skip');
    const cleared = applyDecisionToTree(rows, withDecisions, 'd', 'auto');
    expect(cleared['d']).toBeUndefined();
    expect(cleared['d/cf']).toBeUndefined();
  });

  it('决策聚合统计冲突解决进度', () => {
    const rows = mixedRows();
    const none = mergeDecisionStats(rows, {});
    expect(none.conflicts).toBe(2); // d/cf 叶 + d 目录行
    expect(none.conflictsResolved).toBe(0);
    expect(none.orphans).toBe(1);
    const decided = mergeDecisionStats(rows, { 'd/cf': 'takeLeft' });
    expect(decided.conflictsResolved).toBe(1);
    expect(decided.explicitDecisions).toBe(1);
  });

  it('目录决策继承下的冲突行计入已解决', () => {
    const rows = mixedRows();
    const stats = mergeDecisionStats(rows, { d: 'takeLeft' });
    // d 目录行 + d/cf 叶均在 takeLeft 继承下 → 2 冲突行已解决
    expect(stats.conflictsResolved).toBe(2);
  });
});

describe('folder-merge/mergeRows 计划预览（以引擎 ops 为权威）', () => {
  const ops = (...t: [DirMergeOpKind, string, string][]): DirMergePlannedOp[] =>
    t.map(([kind, path, reason]) => ({ kind, path, reason }));

  it('动作/理由/顺序全部取自引擎计划，客户端不再推导', () => {
    const rows = mixedRows();
    const plan = ops(
      ['takeLeft', 'd/lc', '仅左改 → 取左'],
      ['takeLeft', 'lo', '左孤立并入目标'],
      ['deleteTarget', 'rc', '跟随右侧删除'],
    );
    const preview = mergePreviewFromPlan(rows, {}, plan);
    expect(preview.map((p) => p.path)).toEqual(['d/lc', 'lo', 'rc']);
    expect(preview.map((p) => p.op)).toEqual(['takeLeft', 'takeLeft', 'deleteTarget']);
    expect(preview.map((p) => p.reason)).toEqual(['仅左改 → 取左', '左孤立并入目标', '跟随右侧删除']);
    expect(preview[2].warning).toBe(true);
    expect(preview.every((p) => p.actionable)).toBe(true);
  });

  it('引擎未计划的路径不进预览（即使本地行存在且决策为 auto）', () => {
    const rows = mixedRows();
    const preview = mergePreviewFromPlan(rows, {}, ops(['takeLeft', 'lo', '左孤立并入目标']));
    expect(preview.map((p) => p.path)).toEqual(['lo']);
    // same/rc/d/cf 等行本地都在，但引擎没计划 → 不得出现在执行清单里
    expect(preview.some((p) => p.path === 'same')).toBe(false);
    expect(preview.some((p) => p.path === 'd/cf')).toBe(false);
  });

  it('join 行元数据与有效决策（含祖先继承）', () => {
    const rows = mixedRows();
    const preview = mergePreviewFromPlan(rows, { d: 'takeLeft' }, ops(['takeLeft', 'd/cf', '决策:取左']));
    const cf = preview[0];
    expect(cf.name).toBe('cf');
    expect(cf.kind).toBe('file');
    expect(cf.category).toBe('conflict');
    expect(cf.decision).toBe('takeLeft');
  });

  it('引擎显式 keep 项：占位不可执行、无删除告警', () => {
    const preview = mergePreviewFromPlan(mixedRows(), { lo: 'skip' }, ops(['keep', 'lo', '决策:跳过']));
    expect(preview[0].op).toBe('keep');
    expect(preview[0].actionable).toBe(false);
    expect(preview[0].warning).toBe(false);
    expect(preview[0].decision).toBe('skip');
  });

  it('预览统计含 keep（与确认框统计行同口径）', () => {
    const preview = mergePreviewFromPlan(
      mixedRows(),
      {},
      ops(['takeLeft', 'd/lc', 'a'], ['takeLeft', 'lo', 'b'], ['deleteTarget', 'rc', 'c'], ['keep', 'same', 'd']),
    );
    const stats = summarizeMergePlan(preview);
    expect(stats).toMatchObject({ takeLeft: 2, takeRight: 0, deleteTarget: 1, keep: 1, total: 4 });
  });

  it('空计划 → 空预览（planEmpty 分支）', () => {
    expect(mergePreviewFromPlan(mixedRows(), {}, [])).toEqual([]);
  });
});

describe('folder-merge/mergeRows 执行映射与报告导出', () => {

  it('导出文本报告', () => {
    const text = exportMergeReportText(
      { target: 'right', base: '/B', left: '/L', right: '/R' },
      { total: 3, copied: 2, deleted: 1, skipped: 0, failed: 0, cancelled: false, details: [{ kind: 'takeLeft', path: 'a', reason: '仅左改 → 取左', status: 'copied' }] },
    );
    expect(text).toContain('文件夹合并报告');
    expect(text).toContain('合并目标：右侧');
    expect(text).toContain('复制 2 / 删除 1');
    expect(text).toContain('[copied] takeLeft a');
  });
});

describe('folder-merge/mergeRows 决策标签全集', () => {
  it('五类决策均可表达（末次 auto 清除显式项）', () => {
    const all: DirMergeDecision[] = ['takeLeft', 'takeRight', 'both', 'skip', 'auto'];
    const rows = mixedRows();
    let decisions: DecisionMap = {};
    for (const d of all) {
      decisions = applyDecisionToTree(rows, decisions, 'same', d);
    }
    expect(decisions['same']).toBeUndefined(); // 最后 auto 清除
    decisions = applyDecisionToTree(rows, decisions, 'same', 'skip');
    expect(decisions['same']).toBe('skip');
  });
});
