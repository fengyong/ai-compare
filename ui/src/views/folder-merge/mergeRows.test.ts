import { describe, expect, it } from 'vitest';
import type { DirMergeRow, MergeDecision, MergeTarget } from '../../protocol/types';
import {
  applyDecisionToTree,
  autoDefaultOp,
  buildMergePreview,
  defaultExpanded,
  effectiveDecision,
  exportMergeReportText,
  hasMergeChildren,
  mergeDecisionStats,
  mergeStateClass,
  orderedMergeRows,
  previewToMergeOps,
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

describe('folder-merge/mergeRows 计划预览（默认决策表镜像）', () => {
  it('目标右：仅左改取左、右改保持、三方一致保持', () => {
    const rows = mixedRows();
    const preview = buildMergePreview(rows, {}, 'right');
    const byPath = new Map(preview.map((p) => [p.path, p]));
    expect(byPath.get('d/lc')!.op).toBe('takeLeft');
    expect(byPath.get('rc')!.op).toBe('keep');
    expect(byPath.get('same')!.op).toBe('keep');
    expect(byPath.get('lo')!.op).toBe('takeLeft');
    expect(byPath.get('d/cf')!.op).toBe('keep');
  });

  it('目标左：右改取右', () => {
    const rows = mixedRows();
    const preview = buildMergePreview(rows, {}, 'left');
    const byPath = new Map(preview.map((p) => [p.path, p]));
    expect(byPath.get('rc')!.op).toBe('takeRight');
    expect(byPath.get('d/lc')!.op).toBe('keep');
    expect(byPath.get('lo')!.op).toBe('keep');
  });

  it('输出目标：三方一致也复制、冲突/两侧已删不动', () => {
    const rows = [
      ...mixedRows(),
      mrow('gone', { category: 'baseOnly', rowState: 'orphan' }),
    ];
    const preview = buildMergePreview(rows, {}, 'output');
    const byPath = new Map(preview.map((p) => [p.path, p]));
    expect(byPath.get('same')!.op).toBe('takeLeft');
    expect(byPath.get('gone')!.op).toBe('keep');
    expect(byPath.get('d/cf')!.op).toBe('keep');
    expect(byPath.get('lo')!.op).toBe('takeLeft');
  });

  it('删除跟随：目标左时 deletedRight 产生 deleteTarget', () => {
    const rows = [mrow('dr', { category: 'deletedRight', rowState: 'auto' })];
    const preview = buildMergePreview(rows, {}, 'left');
    expect(preview[0].op).toBe('deleteTarget');
    expect(preview[0].warning).toBe(true);
    const previewRight = buildMergePreview(rows, {}, 'right');
    expect(previewRight[0].op).toBe('keep');
  });

  it('冲突行显式取左覆盖默认', () => {
    const rows = mixedRows();
    const preview = buildMergePreview(rows, { 'd/cf': 'takeLeft' }, 'right');
    const cf = preview.find((p) => p.path === 'd/cf')!;
    expect(cf.op).toBe('takeLeft');
    expect(cf.reason).toContain('取左');
    expect(cf.actionable).toBe(true);
  });

  it('skip 决策产生 keep 占位行', () => {
    const rows = mixedRows();
    const preview = buildMergePreview(rows, { lo: 'skip' }, 'right');
    const lo = preview.find((p) => p.path === 'lo')!;
    expect(lo.op).toBe('keep');
    expect(lo.reason).toContain('跳过');
    expect(lo.actionable).toBe(false);
  });

  it('目录 both 决策 = 递归合并（子项按各自决策）', () => {
    const rows = mixedRows();
    const preview = buildMergePreview(rows, { d: 'both' }, 'right');
    const d = preview.find((p) => p.path === 'd')!;
    expect(d.op).toBe('keep');
    expect(d.reason).toContain('递归');
    // 子项 cf 仍在（both 继承下冲突默认人工）
    expect(preview.some((p) => p.path === 'd/cf')).toBe(true);
  });

  it('预览统计与可执行动作行', () => {
    const rows = mixedRows();
    const preview = buildMergePreview(rows, { 'd/cf': 'takeLeft' }, 'right');
    const stats = summarizeMergePlan(preview);
    expect(stats.takeLeft).toBeGreaterThanOrEqual(3); // d/cf + d/lc + lo
    const ops = previewToMergeOps(preview);
    expect(ops.every((o) => o.kind !== 'keep')).toBe(true);
    expect(ops.some((o) => o.path === 'd/cf' && o.kind === 'takeLeft')).toBe(true);
  });

  it('autoDefaultOp 全分类覆盖（10 分类 × 3 目标抽样）', () => {
    const targets: MergeTarget[] = ['left', 'right', 'output'];
    for (const t of targets) {
      for (const cat of ['same', 'leftChanged', 'rightChanged', 'bothSame', 'deletedLeft', 'deletedRight', 'conflict', 'leftOnly', 'rightOnly', 'baseOnly'] as const) {
        const row = mrow('x', { category: cat, rowState: 'auto' });
        const r = autoDefaultOp(row, t);
        expect(['takeLeft', 'takeRight', 'deleteTarget', 'keep']).toContain(r.op);
        expect(r.reason.length).toBeGreaterThan(0);
      }
    }
  });
});

describe('folder-merge/mergeRows 执行映射与报告导出', () => {

  it('导出文本报告', () => {
    const text = exportMergeReportText(
      { target: 'right', base: '/B', left: '/L', right: '/R' },
      { total: 3, copied: 2, deleted: 1, skipped: 0, failed: 0, cancelled: false, details: [{ kind: 'takeLeft', path: 'a', status: 'copied', detail: null }] },
    );
    expect(text).toContain('文件夹合并报告');
    expect(text).toContain('合并目标：右侧');
    expect(text).toContain('复制 2 / 删除 1');
    expect(text).toContain('[copied] takeLeft a');
  });
});

describe('folder-merge/mergeRows 决策标签全集', () => {
  it('五类决策均可表达（末次 auto 清除显式项）', () => {
    const all: MergeDecision[] = ['takeLeft', 'takeRight', 'both', 'skip', 'auto'];
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
