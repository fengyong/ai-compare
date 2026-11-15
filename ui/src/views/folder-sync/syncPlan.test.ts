import { describe, expect, it } from 'vitest';
import type { DirRow, PlannedOperation } from '../../protocol/types';
import { plannedToOp } from '../folder-compare/dirOps';
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
function row(
  id: string,
  status: DirRow['status'],
  lm = 0,
  rm = 0,
  kind: DirRow['kind'] = 'file',
): DirRow {
  return {
    id,
    name: id.split('/').pop() ?? id,
    kind,
    left:
      status === 'rightOnly' ? undefined : { size: 1, mtimeMs: lm, unixMode: 0, uid: 0, gid: 0 },
    right:
      status === 'leftOnly' ? undefined : { size: 1, mtimeMs: rm, unixMode: 0, uid: 0, gid: 0 },
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
  const mirrorPlan = [
    op('copyLeftToRight', 'lonly.txt'),
    op('deleteRight', 'ronly.txt'),
    op('copyLeftToRight', 'diff.txt'),
  ];

  it('默认策略全部保留', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, DEFAULT_SYNC_STRATEGY);
    expect(r.ops).toHaveLength(3);
    expect(r.skipped).toHaveLength(0);
  });

  it('保留孤立项开关拦下镜像删除', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, {
      ...DEFAULT_SYNC_STRATEGY,
      deleteOrphans: false,
    });
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
    const r = applyStrategyToPlan(mirrorPlan, rows, {
      ...DEFAULT_SYNC_STRATEGY,
      readonlySide: 'right',
    });
    expect(r.ops).toHaveLength(0);
    expect(r.skipped).toHaveLength(3);
    expect(r.skipped.every((p) => p.reason.includes('readonlySide'))).toBe(true);
  });

  it('只读左不拦下写右操作', () => {
    const r = applyStrategyToPlan(mirrorPlan, rows, {
      ...DEFAULT_SYNC_STRATEGY,
      readonlySide: 'left',
    });
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
    const plan = [
      op('copyLeftToRight', 'emptydir'),
      op('copyLeftToRight', 'full'),
      op('copyLeftToRight', 'full/a.txt'),
      op('copyLeftToRight', 'lonly.txt'),
    ];
    const r = applyStrategyToPlan(plan, dirRows, {
      ...DEFAULT_SYNC_STRATEGY,
      syncEmptyDirs: false,
    });
    expect(r.ops.map((o) => o.path)).toEqual(['full', 'full/a.txt', 'lonly.txt']);
    expect(r.skipped.map((o) => o.path)).toEqual(['emptydir']);
    expect(r.skipped[0].reason).toContain('skipEmptyDir');
  });

  it('空目录开关开启时不跳过目录', () => {
    const dirRows = [row('emptydir', 'leftOnly', 0, 0, 'dir')];
    const r = applyStrategyToPlan(
      [op('copyLeftToRight', 'emptydir')],
      dirRows,
      DEFAULT_SYNC_STRATEGY,
    );
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
    const planRows = buildSyncPlanRows(
      [op('copyLeftToRight', 'lonly.txt')],
      [op('deleteRight', 'ronly.txt')],
      rows,
      {},
    );
    const skipped = planRows.find((p) => p.path === 'ronly.txt')!;
    expect(skipped.effectiveOp).toBe('keep');
    expect(skipped.source).toBe('strategySkip');
  });

  it('逐项改写覆盖预设动作（FR-SYNC-003）', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, {
      'lonly.txt': 'keep',
    });
    expect(planRows[0].effectiveOp).toBe('keep');
    expect(planRows[0].source).toBe('userKeep');
    const rows2 = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, {
      'lonly.txt': 'copyRightToLeft',
    });
    expect(rows2[0].effectiveOp).toBe('copyRightToLeft');
    expect(rows2[0].direction).toBe('r2l');
  });

  it('改写 keep 不进入执行批次', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, {
      'lonly.txt': 'keep',
    });
    expect(planRowsToExecuteOps(planRows)).toHaveLength(0);
  });

  it('改写为可执行动作映射到 DirExecuteOp（复用 plannedToOp 口径）', () => {
    const planRows = buildSyncPlanRows([op('copyLeftToRight', 'lonly.txt')], [], rows, {
      'lonly.txt': 'deleteLeft',
    });
    const ops = planRowsToExecuteOps(planRows);
    expect(ops).toEqual([{ kind: 'delete', path: 'lonly.txt', side: 'left' }]);
  });

  it('统计聚合计数各动作', () => {
    const planRows = buildSyncPlanRows(
      [
        op('copyLeftToRight', 'lonly.txt'),
        op('deleteRight', 'ronly.txt'),
        op('copyRightToLeft', 'diff.txt'),
      ],
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
      failed: [
        {
          op: { kind: 'copy', path: 'c.txt', side: 'left', targetSide: 'right' },
          status: 'failed',
          detail: '源缺失',
        },
      ],
      skipped: [],
      cancelled: false,
      dryRun: false,
    });
    expect(report).toMatchObject({
      copied: 1,
      deleted: 1,
      touched: 0,
      skipped: 0,
      failed: 1,
      total: 3,
    });
    expect(report.details[0].status).toBe('failed');
    expect(report.details[report.details.length - 1].path).toBe('b.txt');
  });

  it('cancelled 标记传递', () => {
    const report = reportFromExecute({
      total: 0,
      succeeded: [],
      failed: [],
      skipped: [],
      cancelled: true,
      dryRun: false,
    });
    expect(report.cancelled).toBe(true);
  });

  it('导出文本报告含预设/策略/统计/明细', () => {
    const strategy: SyncStrategyUi = {
      deleteOrphans: false,
      leftWins: true,
      readonlySide: 'left',
      syncEmptyDirs: false,
    };
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
    const text = exportSyncReportText(
      { preset: '镜像 左→右', strategy, left: '/L', right: '/R', dryRun: false },
      report,
    );
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
      {
        preset: '双向更新',
        strategy: DEFAULT_SYNC_STRATEGY,
        left: '/L',
        right: '/R',
        dryRun: true,
      },
      {
        copied: 0,
        deleted: 0,
        touched: 0,
        skipped: 0,
        failed: 0,
        total: 0,
        cancelled: false,
        details: [],
      },
    );
    expect(text).toContain('试运行');
  });
});

// ---------------------------------------------------------------------------
// R3-03：显示 id（rowId）与真实执行地址分离
// ---------------------------------------------------------------------------
describe('R3-03：预览显示真实路径，执行载荷不依赖行 id', () => {
  const flatRow = (): DirRow => ({
    ...row('x.txt#0', 'contentDiff', 200, 100),
    leftPath: 'sub/x.txt',
    rightPath: 'sub/x.txt',
  });

  it('平铺行：预览显示真实地址，执行载荷用真实 rel 而非 #i 行 id', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'x.txt#0',
        reason: 'preset:mirrorLeftToRight',
        sourcePath: 'sub/x.txt',
        targetPath: 'sub/x.txt',
        displayPath: 'sub/x.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [flatRow()], {});
    // 关联键（选中/逐项改写）仍是行 id
    expect(planRows[0].path).toBe('x.txt#0');
    // 计划预览显示引擎解析出的真实执行地址
    expect(planRows[0].displayPath).toBe('sub/x.txt');
    const ops = planRowsToExecuteOps(planRows);
    expect(ops).toEqual([
      {
        kind: 'copy',
        path: 'sub/x.txt',
        side: 'left',
        targetSide: 'right',
        targetPath: 'sub/x.txt',
      },
    ]);
    expect(ops[0].path).not.toContain('#');
  });

  it('左右异名：执行载荷分别携带源/目标真实路径', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'a.txt',
        targetPath: 'b.txt',
        displayPath: 'b.txt',
      },
    ];
    const alignedRow: DirRow = {
      ...row('a.txt', 'contentDiff', 200, 100),
      leftPath: 'a.txt',
      rightPath: 'b.txt',
    };
    const planRows = buildSyncPlanRows(plan, [], [alignedRow], {});
    expect(planRows[0].displayPath).toBe('b.txt');
    const ops = planRowsToExecuteOps(planRows);
    expect(ops[0].path).toBe('a.txt');
    expect(ops[0].targetPath).toBe('b.txt');
  });

  it('逐项改写（override）后真实路径字段不丢失', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'x.txt#0',
        reason: 'test',
        sourcePath: 'sub/x.txt',
        targetPath: 'sub/x.txt',
        displayPath: 'sub/x.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [flatRow()], {
      'x.txt#0': 'copyRightToLeft',
    });
    const ops = planRowsToExecuteOps(planRows);
    expect(ops[0].kind).toBe('copy');
    expect(ops[0].side).toBe('right');
    expect(ops[0].path).toBe('sub/x.txt');
    expect(ops[0].path).not.toContain('#');
  });

  it('leftWins 改向后源/目标真实地址同步换向', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyRightToLeft',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'b.txt',
        targetPath: 'a.txt',
        displayPath: 'a.txt',
      },
    ];
    const alignedRow: DirRow = {
      ...row('a.txt', 'contentDiff', 100, 200),
      leftPath: 'a.txt',
      rightPath: 'b.txt',
    };
    const r = applyStrategyToPlan(plan, [alignedRow], {
      ...DEFAULT_SYNC_STRATEGY,
      leftWins: true,
    });
    expect(r.ops[0].op).toBe('copyLeftToRight');
    expect(r.ops[0].sourcePath).toBe('a.txt');
    expect(r.ops[0].targetPath).toBe('b.txt');
    expect(r.ops[0].displayPath).toBe('b.txt');
    const ops = planRowsToExecuteOps(buildSyncPlanRows(r.ops, r.skipped, [alignedRow], {}));
    expect(ops[0].path).toBe('a.txt');
    expect(ops[0].targetPath).toBe('b.txt');
  });

  it('delete/touch 使用 operationPath / sourcePath，不回落到行 id', () => {
    const del: PlannedOperation = {
      op: 'deleteLeft',
      path: 'x.txt#1',
      reason: 'test',
      operationPath: 'sub/only.txt',
      displayPath: 'sub/only.txt',
    };
    const tch: PlannedOperation = {
      op: 'touchLeft',
      path: 'x.txt#0',
      reason: 'test',
      sourcePath: 'other/b.txt',
      operationPath: 'sub/x.txt',
      displayPath: 'sub/x.txt',
    };
    expect(plannedToOp(del)).toEqual({ kind: 'delete', path: 'sub/only.txt', side: 'left' });
    expect(plannedToOp(tch)).toEqual({
      kind: 'touch',
      path: 'sub/x.txt',
      side: 'left',
      mtimeMs: 0,
      targetPath: 'other/b.txt',
    });
  });

  // ------------------------------------------------------------------
  // R3-03（1B）：逐项改写换向必须重新派生真实侧地址
  // ------------------------------------------------------------------

  /** 异名手动对齐行（左右 rel 不同） */
  const pairedRow = (): DirRow => ({
    ...row('a.txt', 'contentDiff', 200, 100),
    leftPath: 'a.txt',
    rightPath: 'b.txt',
  });

  it('异名配对行改写为右→左：源/目标必须换到右侧/左侧真实路径', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'a.txt',
        targetPath: 'b.txt',
        displayPath: 'b.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [pairedRow()], {
      'a.txt': 'copyRightToLeft',
    });
    expect(planRows[0].effectiveOp).toBe('copyRightToLeft');
    expect(planRows[0].sourcePath).toBe('b.txt');
    expect(planRows[0].targetPath).toBe('a.txt');
    const ops = planRowsToExecuteOps(planRows);
    expect(ops[0].side).toBe('right');
    expect(ops[0].path).toBe('b.txt');
    expect(ops[0].targetPath).toBe('a.txt');
  });

  it('异名配对行改写为删除：operationPath 必须是该侧真实路径', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'a.txt',
        targetPath: 'b.txt',
        displayPath: 'b.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [pairedRow()], {
      'a.txt': 'deleteLeft',
    });
    expect(planRows[0].operationPath).toBe('a.txt');
    expect(planRows[0].targetPath).toBeUndefined();
    expect(planRows[0].sourcePath).toBeUndefined();
    const ops = planRowsToExecuteOps(planRows);
    expect(ops[0]).toEqual({ kind: 'delete', path: 'a.txt', side: 'left' });

    const planRows2 = buildSyncPlanRows(plan, [], [pairedRow()], {
      'a.txt': 'deleteRight',
    });
    expect(planRows2[0].operationPath).toBe('b.txt');
    const ops2 = planRowsToExecuteOps(planRows2);
    expect(ops2[0]).toEqual({ kind: 'delete', path: 'b.txt', side: 'right' });
  });

  it('touch 基线计划改写为复制：源/目标按当前行真实路径重新派生', () => {
    // 基线是 touchLeft（source=右侧、operation=左侧），用户改写为右→左复制：
    // 地址必须来自当前行的左右真实路径，不能沿用 touch 的旧载荷。
    const plan: PlannedOperation[] = [
      {
        op: 'touchLeft',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'b.txt',
        operationPath: 'a.txt',
        displayPath: 'a.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [pairedRow()], {
      'a.txt': 'copyRightToLeft',
    });
    expect(planRows[0].sourcePath).toBe('b.txt');
    expect(planRows[0].targetPath).toBe('a.txt');
    expect(planRows[0].operationPath).toBeUndefined();
    const ops = planRowsToExecuteOps(planRows);
    expect(ops[0].kind).toBe('copy');
    expect(ops[0].side).toBe('right');
    expect(ops[0].path).toBe('b.txt');
    expect(ops[0].targetPath).toBe('a.txt');
  });

  it('auto 保持引擎计划地址（不重新派生）', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'touchLeft',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'b.txt',
        operationPath: 'a.txt',
        displayPath: 'a.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [pairedRow()], {});
    expect(planRows[0].sourcePath).toBe('b.txt');
    expect(planRows[0].operationPath).toBe('a.txt');
    expect(planRows[0].effectiveOp).toBe('touchLeft');
  });

  it('预览显示路径与最终执行地址一致（改写后）', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'a.txt',
        targetPath: 'b.txt',
        displayPath: 'b.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [pairedRow()], {
      'a.txt': 'copyRightToLeft',
    });
    const ops = planRowsToExecuteOps(planRows);
    expect(planRows[0].displayPath).toBe(ops[0].targetPath);
  });

  it('用户 keep 后无执行载荷', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'a.txt',
        reason: 'test',
        sourcePath: 'a.txt',
        targetPath: 'b.txt',
        displayPath: 'b.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [pairedRow()], { 'a.txt': 'keep' });
    expect(planRows[0].effectiveOp).toBe('keep');
    expect(planRows[0].sourcePath).toBeUndefined();
    expect(planRows[0].targetPath).toBeUndefined();
    expect(planRows[0].operationPath).toBeUndefined();
    expect(planRowsToExecuteOps(planRows)).toHaveLength(0);
  });

  it('平铺行（name#i）改写后执行地址仍是真实 rel，不含 #', () => {
    const plan: PlannedOperation[] = [
      {
        op: 'copyLeftToRight',
        path: 'x.txt#0',
        reason: 'test',
        sourcePath: 'sub/x.txt',
        targetPath: 'sub/x.txt',
        displayPath: 'sub/x.txt',
      },
    ];
    const planRows = buildSyncPlanRows(plan, [], [flatRow()], {
      'x.txt#0': 'deleteRight',
    });
    expect(planRows[0].operationPath).toBe('sub/x.txt');
    const ops = planRowsToExecuteOps(planRows);
    expect(ops[0].path).toBe('sub/x.txt');
    expect(ops[0].path).not.toContain('#');
  });
});
