import { useEffect, useMemo, useRef, useState } from 'react';
import {
  DEFAULT_DIR_CRITERIA,
  type PlannedOpKind,
  type PlannedOperation,
  type SyncPreset,
} from '../../protocol/types';
import type { Tab } from '../../shell/store';
import { useShell } from '../../shell/store';
import {
  cancelEngineRequest,
  executeOperations,
  openFolder,
  planOperations,
  trackSession,
} from '../../ipc/engineProcess';
import { S } from '../../strings';
import { ensureDirCacheAttached } from '../folder-compare/dirRowCache';
import {
  applyStrategyToPlan,
  buildSyncPlanRows,
  DEFAULT_SYNC_STRATEGY,
  exportSyncReportText,
  planRowsToExecuteOps,
  reportFromExecute,
  ROW_OVERRIDE_LABELS,
  summarizeSyncPlan,
  type RowOpOverride,
  type SyncExecReport,
  type SyncPlanRow,
  type SyncStrategyUi,
} from './syncPlan';
import { SYNC_PRESETS, SYNC_PRESET_LABELS } from '../folder-compare/dirOps';

/**
 * 文件夹同步视图（FR-SYNC-001~005，P8）：
 * - 上部：左右基文件夹 + 同步方法下拉 + 策略选项（SYNC-004 预设内可配）；
 * - 中部：计划预览表（方向着色/删除警示 + 逐项改写 FR-SYNC-003）；
 * - 确认 → 分批执行（进度条）→ 结果报告页（统计卡 + 明细 + 导出文本报告）。
 *
 * 数据链路：dir/open（复用文件夹比较会话）→ dir/operate 计划 → 客户端
 * 策略施加（与引擎 apply_strategy 同语义；协议携带策略后迁回引擎）→
 * dir/execute 分批执行（每批产生进度推进）。
 */

const OP_LABELS: Record<PlannedOpKind | 'keep', string> = {
  copyLeftToRight: '复制 左→右',
  copyRightToLeft: '复制 右→左',
  deleteLeft: '删除左侧',
  deleteRight: '删除右侧',
  touchLeft: '对时 左',
  touchRight: '对时 右',
  keep: '保持不变',
};

/** 分批执行批大小（进度条粒度；单批过大会失去推进感） */
const EXEC_BATCH_SIZE = 20;

export function FolderSyncView({ tab }: { tab: Tab }) {
  const folder = tab.sync!;
  const setNotice = useShell((st) => st.setNotice);
  const cache = ensureDirCacheAttached(folder.sessionId!);

  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preset, setPreset] = useState<SyncPreset>('mirrorLeftToRight');
  const [strategy, setStrategy] = useState<SyncStrategyUi>(DEFAULT_SYNC_STRATEGY);
  // 阶段状态机：setup（选方法/策略）→ plan（预览确认）→ running → report
  const [phase, setPhase] = useState<'setup' | 'plan' | 'running' | 'report'>('setup');
  const [planRows, setPlanRows] = useState<SyncPlanRow[]>([]);
  const [overrides, setOverrides] = useState<Record<string, RowOpOverride>>({});
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [report, setReport] = useState<SyncExecReport | null>(null);
  const [reportPreset, setReportPreset] = useState('');
  // 取消标记必须放 ref：runSync 是跨多次 await 的长循环，持有的是发起那一帧
  // 的闭包；用 state 的话按钮置 true 后循环里读到的仍是旧值 false → 取消永不生效
  const cancelRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    void cache
      .ensureOpened(() =>
        openFolder({
          sessionId: cache.sessionId,
          left: folder.left,
          right: folder.right,
          depth: null,
          criteria: DEFAULT_DIR_CRITERIA,
          lazyScan: true,
          flat: false,
          nameFilter: null,
          otherFilter: null,
          displayFilter: null,
          manualAlign: null,
        }),
      )
      .then(() => {
        if (!disposed) {
          trackSession(cache.sessionId, 'dir', {
            left: folder.left,
            right: folder.right,
            depth: null,
            criteria: DEFAULT_DIR_CRITERIA,
            lazyScan: true,
            flat: false,
            nameFilter: null,
            otherFilter: null,
            displayFilter: null,
            manualAlign: null,
          });
          setReady(true);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
  }, [cache, folder.left, folder.right]);

  const rows = cache.getRows();
  const visibleRows = useMemo(() => rows.filter((r) => !r.hidden), [rows]);

  // ---- 生成计划（dir/operate -> 策略施加 -> 预览行状态机） ----
  const [basePlan, setBasePlan] = useState<{
    ops: PlannedOperation[];
    skipped: PlannedOperation[];
  } | null>(null);

  const acceptPlan = async (): Promise<void> => {
    try {
      const result = await planOperations({
        sessionId: cache.sessionId,
        preset,
        rules: null,
        paths: null,
      });
      const base = applyStrategyToPlan(result.plan, visibleRows, strategy);
      setBasePlan(base);
      setOverrides({});
      setPlanRows(buildSyncPlanRows(base.ops, base.skipped, visibleRows, {}));
      setPhase('plan');
    } catch (err: unknown) {
      const isMethodNotFound =
        err instanceof Error && 'code' in err && (err as { code?: number }).code === -32601;
      setNotice(
        isMethodNotFound
          ? S.syncView.engineUnsupported
          : S.syncView.opsFailedMsg(err instanceof Error ? err.message : String(err)),
      );
    }
  };

  const stats = summarizeSyncPlan(planRows);
  const pendingOps = planRowsToExecuteOps(planRows);

  const overrideRow = (path: string, value: RowOpOverride): void => {
    const nextOverrides = { ...overrides, [path]: value };
    setOverrides(nextOverrides);
    if (basePlan) {
      setPlanRows(buildSyncPlanRows(basePlan.ops, basePlan.skipped, visibleRows, nextOverrides));
    }
  };

  // ---- 立即同步（分批执行 → 进度 → 报告） ----
  const runSync = async (): Promise<void> => {
    setPhase('running');
    cancelRef.current = false;
    setProgress({ done: 0, total: pendingOps.length });
    const report: SyncExecReport = {
      copied: 0,
      deleted: 0,
      touched: 0,
      skipped: 0,
      failed: 0,
      total: pendingOps.length,
      cancelled: false,
      details: [],
    };
    for (let i = 0; i < pendingOps.length; i += EXEC_BATCH_SIZE) {
      if (cancelRef.current) {
        const rest = pendingOps.length - i;
        report.cancelled = true;
        report.skipped += rest;
        for (const op of pendingOps.slice(i))
          report.details.push({
            kind: op.kind,
            path: op.path,
            status: 'skipped',
            detail: '已取消',
          });
        break;
      }
      const batch = pendingOps.slice(i, i + EXEC_BATCH_SIZE);
      try {
        const result = await executeOperations({ sessionId: cache.sessionId, ops: batch });
        const part = reportFromExecute({ ...result, total: batch.length });
        report.copied += part.copied;
        report.deleted += part.deleted;
        report.touched += part.touched;
        report.skipped += part.skipped;
        report.failed += part.failed;
        report.details.push(...part.details);
      } catch (err: unknown) {
        const isMethodNotFound =
          err instanceof Error && 'code' in err && (err as { code?: number }).code === -32601;
        if (isMethodNotFound) {
          setNotice(S.syncView.engineUnsupported);
        } else {
          setNotice(S.syncView.opsFailedMsg(err instanceof Error ? err.message : String(err)));
        }
        report.failed += batch.length;
        for (const op of batch)
          report.details.push({
            kind: op.kind,
            path: op.path,
            status: 'failed',
            detail: '执行请求失败',
          });
      }
      setProgress({
        done: Math.min(i + EXEC_BATCH_SIZE, pendingOps.length),
        total: pendingOps.length,
      });
    }
    setReport(report);
    setReportPreset(SYNC_PRESET_LABELS[preset]);
    setPhase('report');
  };

  const exportReport = async (): Promise<void> => {
    if (!report) return;
    const text = exportSyncReportText(
      { preset: reportPreset, strategy, left: folder.left, right: folder.right, dryRun: false },
      report,
    );
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // 剪贴板不可用时仅下载
    }
    const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'sync-report.txt';
    a.click();
    URL.revokeObjectURL(url);
    setNotice(S.syncView.reportExported);
  };

  if (error) {
    return (
      <div className="aic-folder">
        <div className="aic-error-box">
          <b>{S.syncView.openFail}</b>
          <pre>{error}</pre>
          <button onClick={() => setError(null)}>关闭</button>
        </div>
      </div>
    );
  }

  return (
    <div className="aic-folder aic-sync">
      <div className="aic-folder-toolbar">
        <span className="aic-folder-paths" title={`${folder.left} ⇄ ${folder.right}`}>
          {folder.left} ⇄ {folder.right}
        </span>
        {ready && <span className="dim">{visibleRows.length} 行已加载</span>}
      </div>

      {/* 阶段 1：方法 + 策略（FR-SYNC-001/004） */}
      {phase === 'setup' && (
        <div className="aic-sync-setup">
          <div className="aic-sync-row">
            <label>{S.syncView.methodLabel}</label>
            <select
              className="aic-mini-select"
              value={preset}
              disabled={!ready}
              onChange={(e) => setPreset(e.target.value as SyncPreset)}
            >
              {SYNC_PRESETS.map((p) => (
                <option key={p} value={p}>
                  {SYNC_PRESET_LABELS[p]}
                </option>
              ))}
            </select>
            <button className="mini primary" disabled={!ready} onClick={() => void acceptPlan()}>
              {S.syncView.planAccept}
            </button>
          </div>
          <div className="aic-sync-row">
            <label>{S.syncView.strategyLabel}</label>
            <label className="chk">
              <input
                type="checkbox"
                checked={!strategy.deleteOrphans}
                onChange={(e) => setStrategy({ ...strategy, deleteOrphans: !e.target.checked })}
              />
              {S.syncView.keepOrphans}
            </label>
            <label className="chk">
              <input
                type="checkbox"
                checked={strategy.leftWins}
                onChange={(e) => setStrategy({ ...strategy, leftWins: e.target.checked })}
              />
              {strategy.leftWins ? S.syncView.leftWins : S.syncView.newerWins}
            </label>
            <select
              className="aic-mini-select"
              value={strategy.readonlySide}
              onChange={(e) =>
                setStrategy({
                  ...strategy,
                  readonlySide: e.target.value as SyncStrategyUi['readonlySide'],
                })
              }
            >
              <option value="none">{S.syncView.readonlyNone}</option>
              <option value="left">{S.syncView.readonlyLeft}</option>
              <option value="right">{S.syncView.readonlyRight}</option>
            </select>
            <label className="chk">
              <input
                type="checkbox"
                checked={strategy.syncEmptyDirs}
                onChange={(e) => setStrategy({ ...strategy, syncEmptyDirs: e.target.checked })}
              />
              {S.syncView.syncEmptyDirs}
            </label>
          </div>
        </div>
      )}

      {/* 阶段 2：计划预览（FR-SYNC-003 逐项改写） */}
      {phase === 'plan' && (
        <div className="aic-sync-plan">
          <div className="aic-sync-row">
            <b>{S.syncView.planTitle}</b>
            <span className="dim">{S.syncView.planDesc}</span>
          </div>
          <div className="aic-sync-stats">
            {stats.copyL2R > 0 && (
              <span className="chip l2r">
                {S.syncView.opCopyL2R}× {stats.copyL2R}
              </span>
            )}
            {stats.copyR2L > 0 && (
              <span className="chip r2l">
                {S.syncView.opCopyR2L}× {stats.copyR2L}
              </span>
            )}
            {stats.deleteL > 0 && (
              <span className="chip warn">
                {S.syncView.opDelL}× {stats.deleteL}
              </span>
            )}
            {stats.deleteR > 0 && (
              <span className="chip warn">
                {S.syncView.opDelR}× {stats.deleteR}
              </span>
            )}
            <span className="chip">
              {S.syncView.planKeep}× {stats.keep}
            </span>
          </div>
          {planRows.length === 0 ? (
            <div className="aic-sync-empty">{S.syncView.planEmpty}</div>
          ) : (
            <div className="aic-sync-table">
              <div className="aic-sync-head">
                <span>{S.syncView.planColOp}</span>
                <span>{S.syncView.planColPath}</span>
                <span>{S.syncView.colOverride}</span>
                <span>{S.syncView.planColReason}</span>
              </div>
              <div className="aic-sync-body">
                {planRows.map((pr) => (
                  <div
                    key={pr.path}
                    className={`aic-sync-tr${pr.warning ? ' warn' : ''}${pr.source === 'strategySkip' ? ' skipped' : ''}`}
                  >
                    <span
                      className={
                        pr.direction === 'l2r' ? 'dir-l2r' : pr.direction === 'r2l' ? 'dir-r2l' : ''
                      }
                      title={pr.warning ? S.syncView.planWarnDelete : undefined}
                    >
                      {OP_LABELS[pr.effectiveOp]}
                      {pr.source === 'override' && (
                        <i className="ov-badge">{S.syncView.planSourceUser}</i>
                      )}
                      {pr.source === 'strategySkip' && (
                        <i className="ov-badge">{S.syncView.planSourceStrategy}</i>
                      )}
                    </span>
                    {/* R3-03：预览显示引擎解析出的真实执行地址（与最终落盘路径一致），
                        选中/改写键仍用 pr.path（行 id），两者不得混用 */}
                    <span className="mono" title={pr.displayPath ?? pr.path}>
                      {pr.kind === 'dir' ? '📁 ' : ''}
                      {pr.displayPath ?? pr.path}
                    </span>
                    <span>
                      <select
                        className="aic-mini-select"
                        value={overrides[pr.path] ?? 'auto'}
                        onChange={(e) => overrideRow(pr.path, e.target.value as RowOpOverride)}
                        title={S.syncView.overrideHint}
                      >
                        {(Object.keys(ROW_OVERRIDE_LABELS) as RowOpOverride[]).map((k) => (
                          <option key={k} value={k}>
                            {ROW_OVERRIDE_LABELS[k]}
                          </option>
                        ))}
                      </select>
                    </span>
                    <span className="dim reason">{pr.reason}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
          <div className="aic-sync-actions">
            <button className="mini" onClick={() => setPhase('setup')}>
              {S.syncView.reportBack}
            </button>
            <button
              className="mini primary"
              disabled={pendingOps.length === 0}
              onClick={() => void runSync()}
            >
              {S.syncView.planConfirm}（{pendingOps.length}）
            </button>
          </div>
        </div>
      )}

      {/* 阶段 3：执行进度 */}
      {phase === 'running' && progress && (
        <div className="aic-sync-running">
          <span className="aic-progress">
            <span className="bar">
              <i
                style={{
                  width: `${progress.total === 0 ? 100 : Math.round((progress.done / progress.total) * 100)}%`,
                }}
              />
            </span>
            {progress.total === 0
              ? S.syncView.executing
              : S.syncView.running(progress.done, progress.total)}
          </span>
          <button
            className="mini"
            onClick={() => {
              cancelRef.current = true;
              // R3-22：本地标记只能阻止后续批次；在途的 dir/execute 必须
              // 同时把引擎侧 CancelFlag 置位，否则当前批次仍会跑完。
              // 返回 false 表示引擎未声明该能力（演示引擎只靠本地标记）。
              cancelEngineRequest('dir/execute');
            }}
          >
            取消
          </button>
        </div>
      )}

      {/* 阶段 4：结果报告（统计卡 + 明细 + 导出） */}
      {phase === 'report' && report && (
        <div className="aic-sync-report">
          <div className="aic-sync-row">
            <b>
              {S.syncView.reportTitle}
              {reportPreset ? ` — ${reportPreset}` : ''}
            </b>
            {report.cancelled && <span className="chip warn">{S.syncView.reportCancelled}</span>}
          </div>
          <div className="aic-sync-cards">
            <div className="card">
              <i>{report.copied}</i>
              {S.syncView.reportCopied}
            </div>
            <div className="card">
              <i>{report.deleted}</i>
              {S.syncView.reportDeleted}
            </div>
            <div className="card">
              <i>{report.touched}</i>
              {S.syncView.reportTouched}
            </div>
            <div className="card">
              <i>{report.skipped}</i>
              {S.syncView.reportSkipped}
            </div>
            <div className={`card${report.failed > 0 ? ' bad' : ''}`}>
              <i>{report.failed}</i>
              {S.syncView.reportFailed}
            </div>
          </div>
          <div className="aic-sync-actions">
            <button className="mini" onClick={() => setPhase('setup')}>
              {S.syncView.reportBack}
            </button>
            <button className="mini" onClick={() => void exportReport()}>
              {S.syncView.reportExport}
            </button>
          </div>
          <div className="aic-sync-table small">
            <div className="aic-sync-head">
              <span>{S.syncView.planColOp}</span>
              <span>{S.syncView.planColPath}</span>
              <span>{S.syncView.reportDetail}</span>
            </div>
            <div className="aic-sync-body">
              {report.details.map((d, i) => (
                <div key={`${d.path}-${i}`} className={`aic-sync-tr st-${d.status}`}>
                  <span>{d.status}</span>
                  <span className="mono">{d.path}</span>
                  <span className="dim">{d.detail ?? ''}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {!ready && phase === 'setup' && <div className="aic-sync-empty">{S.engine.connecting}</div>}
    </div>
  );
}
