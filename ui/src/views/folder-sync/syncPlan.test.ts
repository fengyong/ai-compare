import { describe, expect, it } from 'vitest';
import type { DirRow, PlannedOperation } from '../../protocol/types';
import {
  applyStrategyToPlan,
  buildSyncPlanRows,
  DEFAULT_SYNC_STRATEGY,
  exportSyncReportText,
  planRowsToExecuteOps,
  reportFromExecute,
  ROW_OVERRIDE_LABELS,
  summarizeSyncPlan,
  type SyncExecReport,
  type SyncStrategyUi,
} from './syncPlan';

/** 行构造（id/状态/两侧 mtime） */
function row(id: string, status: DirRow['status'], lm = 0, rm = 0, kind: DirRow['kind'] = 'file'): DirRow {
  return {
    id,
    name: id.split('/').pop() ?? id,
    kind,
    left: status === 'rightOnly' ? null : { size: 1, mtimeMs: lm, attribute: null, unixMode: 0, uid: 0, gid: 0 },
    right: status === 'leftOnly' ? null : { size: 1, mtimeMs: rm, attribute: null, unixMode: 0, uid: 0, gid: 0 },
    status,
    hasChildren: kind === 'dir',
    hidden: false,
    suppressed: false,
    leftPath: status === 'rightOnly' ? undefined : id,
    rightPath: status === 'leftOnly' ? undefined : id,
  };
}

function op(kind: PlannedOperation['op'], path: string): PlannedOperation {
  return { op: kind, path, reason: 'test' };
}

describe('folder-sync/syncPlan 策略施加（引擎 apply_strategy 客户端镜像）', () => {
  const rows = [
    row('lonly.txt', 'leftOnly'),
    row('ronly.txt', 'rightOnly'),
    row('diff.txt', 'contentDiff', 200, 100),
  ];
  const mirrorPlan = [op('copyLeftToRight', 'lonly.txt'), op('deleteRight', 'ronly.txt'), op('copyLeftToRight', 'diff.txt')];

  it('默认策略全部保留', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, DEFAULT_SYNC_STRATEGY);
    expect(r.ops).toHaveLength(3);
    expect(r.skipped).toHaveLength(0);
  });

  it('保留孤立项开关拦下镜像删除', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, { ...DEFAULT_SYNC_STRATEGY, deleteOrphans: false });
    expect(r.ops.map((o) => o.path)).toEqual(['lonly.txt', 'diff.txt']);
    expect(r.skipped).toHaveLength(1);
    expect(r.skipped[0].op).toBe('deleteRight');
    expect(r.skipped[0].reason).toContain('keepOrphans');
  });

  it('左侧优先把差异行的右→左复制改向', () => {
    const plan = [op('copyRightToLeft', 'diff.txt')];
    const r = applyStrategyToPlan(plan, rows, { ...DEFAULT_SYNC_STRATEGY, leftWins: true });
    expect(r.ops[0].op).toBe('copyLeftToRight');
    expect(r.ops[0].reason).toContain('leftWins');
  });

  it('左侧优先不影响孤立项方向', () => {
    const plan = [op('copyRightToLeft', 'ronly.txt')];
    const r = applyStrategyToPlan(plan, rows, { ...DEFAULT_SYNC_STRATEGY, leftWins: true });
    expect(r.ops[0].op).toBe('copyRightToLeft');
  });

  it('只读右拦下全部写右操作', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, { ...DEFAULT_SYNC_STRATEGY, readonlySide: 'right' });
    expect(r.ops).toHaveLength(0);
    expect(r.skipped).toHaveLength(3);
    expect(r.skipped.every((p) => p.reason.includes('readonlySide'))).toBe(true);
  });

  it('只读左不拦下写右操作', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, { ...DEFAULT_SYNC_STRATEGY, readonlySide: 'left' });
    expect(r.ops).toHaveLength(3);
    expect(r.skipped).toHaveLength(0);
  });

  it('关闭同步空文件夹跳过孤立叶子目录', () => {
    const dirRows = [
      row('emptydir', 'leftOnly', 0, 0, 'dir'),
      row('full', 'leftOnly', 0, 0, 'dir'),
      row('full/a.txt', 'leftOnly'),
      row('lonly.txt', 'leftOnly'),
    ];
    const plan = [op('copyLeftToRight', 'emptydir'), op('copyLeftToRight', 'full'), op('copyLeftToRight', 'full/a.txt'), op('copyLeftToRight', 'lonly.txt')];
    const r = applyStrategyToPlan(plan, dirRows, { ...DEFAULT_SYNC_STRATEGY, syncEmptyDirs: false });
    expect(r.ops.map((o) => o.path)).toEqual(['full', 'full/a.txt', 'lonly.txt']);
    expect(r.skipped.map((o) => o.path)).toEqual(['emptydir']);
    expect(r.skipped[0].reason).toContain('skipEmptyDir');
  });

  it('空目录开关开启时不跳过目录', () => {
    const dirRows = [row('emptydir', 'leftOnly', 0, 0, 'dir')];
    const r = applyStrategyToPlan([op('copyLeftToRight', 'emptydir')], dirRows, DEFAULT_SYNC_STRATEGY);
    expect(r.ops).toHaveLength(1);
  });

  it('只读侧优先级高于其他策略', () => {
    const r = applyStrategyToPlan([op('deleteRight', 'ronly.txt')], rows, {
      ...DEFAULT_SYNC_STRATEGY,
      deleteOrphans: false,
      readonlySide: 'right',
    });
    expect(r.ops).toHaveLength(0);
    expect(r.skipped[0].reason).toContain('readonlySide');
  });
});

describe('folder-sync/syncPlan 计划表状态机（预览行合成）', () => {
  const rows = [
    row('lonly.txt', 'leftOnly'),
    row('ronly.txt', 'rightOnly'),
    row('diff.txt', 'contentDiff', 200, 100),
  ];

  it('预览行含方向与删除警示', () => {
    const planRows = buildSyncPlanRows(
      [op('copyLeftToRight', 'lonly.txt'), op('deleteRight', 'ronly.txt')],
      [],
      rows,
      {},
    );
    expect(planRows).toHaveLength(2);
    const l2r = planRows.find((p) => p.path === 'lonly.txt')!;
    expect(l2r.direction).toBe('l2r');
    expect(l2r.warning).toBe(false);
    const del = planRows.find((p) => p.path === 'ronly.txt')!;
    expect(del.direction).toBe('none');
    expect(del.warning).toBe(true);
    expect(del.effectiveOp).toBe('deleteRight');
  });

  it('策略跳过行进入预览并标记来源', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [op('deleteRight', 'ronly.txt')], rows, {});
    const skipped = planRows.find((p) => p.path === 'ronly.txt')!;
    expect(skipped.effectiveOp).toBe('keep');
    expect(skipped.source).toBe('strategySkip');
  });

  it('逐项改写覆盖预设动作（FR-SYNC-003）', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, { 'lonly.txt': 'keep' });
    expect(planRows[0].effectiveOp).toBe('keep');
    expect(planRows[0].source).toBe('userKeep');
    const rows2 = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, { 'lonly.txt': 'copyRightToLeft' });
    expect(rows2[0].effectiveOp).toBe('copyRightToLeft');
    expect(rows2[0].direction).toBe('r2l');
  });

  it('改写 keep 不进入执行批次', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, { 'lonly.txt': 'keep' });
    expect(planRowsToExecuteOps(planRows)).toHaveLength(0);
  });

  it('改写为可执行动作映射到 DirExecuteOp（复用 plannedToOp 口径）', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, { 'lonly.txt': 'deleteLeft' });
    const ops = planRowsToExecuteOps(planRows);
    expect(ops).toEqual([{ kind: 'delete', path: 'lonly.txt', side: 'left' }]);
  });

  it('统计聚合计数各动作', () => {
    const planRows = buildSyncPlanRows(
      [op('copyLeftToRight', 'lonly.txt'), op('deleteRight', 'ronly.txt'), op('copyRightToLeft', 'diff.txt')],
      [],
      rows,
      {},
    );
    const s = summarizeSyncPlan(planRows);
    expect(s).toEqual({ copyL2R: 1, copyR2L: 1, deleteL: 0, deleteR: 1, keep: 0, total: 3 });
  });

  it('改写标签全集可渲染', () => {
    expect(Object.keys(ROW_OVERRIDE_LABELS)).toHaveLength(6);
    expect(ROW_OVERRIDE_LABELS.auto).toBe('按预设');
    expect(ROW_OVERRIDE_LABELS.deleteLeft).toBe('删除左侧');
  });

  it('目录行进入预览并保留 kind', () => {
    const dirRows = [row('edir', 'leftOnly', 0, 0, 'dir')];
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'edir')], [], dirRows, {});
    expect(planRows[0].kind).toBe('dir');
    expect(planRows[0].name).toBe('edir');
  });
});

describe('folder-sync/syncPlan 结果报告聚合与导出', () => {
  it('reportFromExecute 按动作类别聚合', () => {
    const report = reportFromExecute({
      total: 3,
      succeeded: [
        { op: { kind: 'copy', path: 'a.txt', side: 'left', targetSide: 'right' }, detail: null },
        { op: { kind: 'delete', path: 'b.txt', side: 'right' }, detail: null },
      ],
      failed: [{ op: { kind: 'copy', path: 'c.txt', side: 'left', targetSide: 'right' }, status: 'failed', detail: '源缺失' }],
      skipped: [],
      cancelled: false,
      dryRun: false,
    });
    expect(report).toMatchObject({ copied: 1, deleted: 1, touched: 0, skipped: 0, failed: 1, total: 3 });
    expect(report.details[0].status).toBe('failed');
    expect(report.details[report.details.length - 1].path).toBe('b.txt');
  });

  it('cancelled 标记传递', () => {
    const report = reportFromExecute({ total: 0, succeeded: [], failed: [], skipped: [], cancelled: true, dryRun: false });
    expect(report.cancelled).toBe(true);
  });

  it('导出文本报告含预设/策略/统计/明细', () => {
    const strategy: SyncStrategyUi = { deleteOrphans: false, leftWins: true, readonlySide: 'left', syncEmptyDirs: false };
    const report: SyncExecReport = {
      copied: 2,
      deleted: 1,
      touched: 0,
      skipped: 1,
      failed: 1,
      total: 5,
      cancelled: false,
      details: [
        { kind: 'copy', path: 'a.txt', status: 'copied', detail: null },
        { kind: 'delete', path: 'b.txt', status: 'failed', detail: '目标不存在' },
      ],
    };
    const text = exportSyncReportText({ preset: '镜像 左→右', strategy, left: '/L', right: '/R', dryRun: false }, report);
    expect(text).toContain('文件夹同步报告');
    expect(text).toContain('预设：镜像 左→右');
    expect(text).toContain('删除孤立项=关');
    expect(text).toContain('左侧优先');
    expect(text).toContain('只读侧=left');
    expect(text).toContain('复制 2 / 删除 1');
    expect(text).toContain('[failed] delete b.txt — 目标不存在');
    expect(text).toContain('[copied] copy a.txt');
  });

  it('试运行报告标注未落盘', () => {
    const text = exportSyncReportText(
      { preset: '双向更新', strategy: DEFAULT_SYNC_STRATEGY, left: '/L', right: '/R', dryRun: true },
      { copied: 0, deleted: 0, touched: 0, skipped: 0, failed: 0, total: 0, cancelled: false, details: [] },
    );
    expect(text).toContain('试运行');
  });
});
