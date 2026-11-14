import { useEffect, useMemo, useRef, useState } from 'react';
import { formatSize, formatTime } from '../../lib/env';
import type { DirMergeRow, DirMergeDecision, DirMergeTarget, DirMergePlannedOp } from '../../protocol/types';
import { mergeExecute, mergePlan, mergeScan, trackSession } from '../../ipc/engineProcess';
import type { DirMergeExecuteResult } from '../../protocol/types';
import type { Tab } from '../../shell/store';
import { useShell } from '../../shell/store';
import { S } from '../../strings';
import {
  applyDecisionToTree,
  decisionAffectsSubtree,
  exportMergeReportText,
  mergeDecisionStats,
  mergePreviewFromPlan,
  mergeStateClass,
  MERGE_CATEGORY_LABELS,
  MERGE_STATE_LABELS,
  orderedMergeRows,
  summarizeMergePlan,
  type DecisionMap,
  type MergePreviewRow,
} from './mergeRows';

/**
 * 文件夹合并视图（FR-DMRG-001~003，P8）：
 * - 三栏目录树（Base/Left/Right 列；行着色：绿=可自动 / 红=冲突 / 灰=孤立 / 暗=一致）；
 * - 冲突/孤立行决策交互（取左/取右/both/跳过；目录决策递归展开到子树）；
 * - 合并计划确认 → 分批执行（进度）→ 结果报告（统计卡 + 明细 + 导出）；
 * - 双击冲突文件行 → 文本合并视图占位（P10 后接真三向文本合并 UI）。
 *
 * 数据链路：dir/mergeScan（三树 diff3 分类）→ dir/mergePlan（决策继承展开，
 * 引擎返回的 ops 即执行清单）→ dir/mergeExecute（部分成功语义 + 日志）。
 */

const DECISION_LABELS: Record<DirMergeDecision, string> = {
  auto: S.mergeView.decAuto,
  takeLeft: S.mergeView.decTakeLeft,
  takeRight: S.mergeView.decTakeRight,
  both: S.mergeView.decBoth,
  skip: S.mergeView.decSkip,
};

/** 分批执行批大小（进度粒度） */
const EXEC_BATCH_SIZE = 20;

const OP_LABELS: Record<string, string> = {
  takeLeft: S.mergeView.opTakeLeft,
  takeRight: S.mergeView.opTakeRight,
  deleteTarget: S.mergeView.opDelete,
  keep: S.mergeView.opKeep,
};

export function FolderMergeView({ tab }: { tab: Tab }) {
  const merge = tab.merge!;
  const setNotice = useShell((st) => st.setNotice);

  const [error, setError] = useState<string | null>(null);
  const [rows, setRows] = useState<DirMergeRow[] | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [target, setTarget] = useState<DirMergeTarget>('right');
  const [outputPath, setOutputPath] = useState('');
  const [decisions, setDecisions] = useState<DecisionMap>({});
  // 阶段：tree（决策）→ plan（确认）→ running → report
  const [phase, setPhase] = useState<'tree' | 'plan' | 'running' | 'report'>('tree');
  const [preview, setPreview] = useState<MergePreviewRow[]>([]);
  const [planOps, setPlanOps] = useState<DirMergePlannedOp[]>([]);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [report, setReport] = useState<DirMergeExecuteResult | null>(null);
  const [textMergeRow, setTextMergeRow] = useState<DirMergeRow | null>(null);
  const sessionId = merge.sessionId;

  // ---- 扫描（dir/mergeScan；失败给引擎未开放提示） ----
  // 扫描幂等守卫（StrictMode 双调用安全）；手动刷新强制重扫
  const scanPromiseRef = useRef<Promise<void> | null>(null);
  const doScan = async (): Promise<void> => {
    setError(null);
    try {
      const result = await mergeScan({ sessionId, base: merge.base, left: merge.left, right: merge.right });
      trackSession(sessionId, 'merge', { base: merge.base, left: merge.left, right: merge.right });
      setRows(result.rows);
      setExpanded(new Set(result.rows.filter((r) => r.kind === 'dir').map((r) => r.relPath)));
      setPhase('tree');
    } catch (err: unknown) {
      const isMethodNotFound = err instanceof Error && 'code' in err && (err as { code?: number }).code === -32601;
      setError(isMethodNotFound ? S.mergeView.engineUnsupported : err instanceof Error ? err.message : String(err));
    }
  };
  const scan = (force = false): Promise<void> => {
    if (force) scanPromiseRef.current = null;
    scanPromiseRef.current ??= doScan();
    return scanPromiseRef.current;
  };

  useEffect(() => {
    void scan();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, merge.base, merge.left, merge.right]);

  const visibleRows = useMemo(() => (rows ? orderedMergeRows(rows, expanded) : []), [rows, expanded]);
  const decisionStats = useMemo(() => (rows ? mergeDecisionStats(rows, decisions) : null), [rows, decisions]);

  const toggleDir = (row: DirMergeRow): void => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(row.relPath)) next.delete(row.relPath);
      else next.add(row.relPath);
      return next;
    });
  };

  const setDecision = (row: DirMergeRow, decision: DirMergeDecision): void => {
    if (decisionAffectsSubtree(rows ?? [], row.relPath)) {
      setNotice(`${row.name}: ${S.mergeView.decHintDir}`);
    }
    setDecisions((prev) => applyDecisionToTree(rows ?? [], prev, row.relPath, decision));
  };

  // ---- 生成合并计划（dir/mergePlan；引擎返回的 ops 即执行清单） ----
  // 目标/输出路径变更后旧计划立即失效：否则会把按旧目标生成的 ops 执行到新目标
  const invalidatePlan = (): void => {
    setPlanOps([]);
    setPreview([]);
    setPhase('tree');
  };

  const buildPlan = async (): Promise<void> => {
    if (target === 'output' && outputPath.trim() === '') {
      setNotice(S.mergeView.outputPathRequired);
      return;
    }
    try {
      const result = await mergePlan({
        sessionId,
        target,
        decisions,
        ...(target === 'output' ? { outputPath: outputPath.trim() } : {}),
      });
      setPlanOps(result.ops);
      setPreview(mergePreviewFromPlan(rows ?? [], decisions, result.ops));
      setPhase('plan');
    } catch (err: unknown) {
      setNotice(err instanceof Error ? err.message : String(err));
    }
  };

  const planStats = summarizeMergePlan(preview);
  const pendingOps = planOps.filter((o) => o.kind !== 'keep');

  // ---- 执行合并（分批 dir/mergeExecute → 进度 → 报告） ----
  const runMerge = async (): Promise<void> => {
    setPhase('running');
    setProgress({ done: 0, total: pendingOps.length });
    const report: DirMergeExecuteResult = {
      sessionId,
      target,
      total: pendingOps.length,
      copied: 0,
      deleted: 0,
      touched: 0,
      skipped: 0,
      failed: 0,
      cancelled: false,
      details: [],
      log: [],
    };
    for (let i = 0; i < pendingOps.length; i += EXEC_BATCH_SIZE) {
      const batch = pendingOps.slice(i, i + EXEC_BATCH_SIZE);
      try {
        const result = await mergeExecute({
          sessionId,
          target,
          ops: batch,
          ...(target === 'output' ? { outputPath: outputPath.trim() } : {}),
        });
        report.copied += result.copied;
        report.deleted += result.deleted;
        report.touched += result.touched;
        report.skipped += result.skipped;
        report.failed += result.failed;
        report.cancelled ||= result.cancelled;
        report.details.push(...result.details);
        report.log.push(...result.log.slice(1));
      } catch (err: unknown) {
        report.failed += batch.length;
        for (const op of batch) report.details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'failed', detail: err instanceof Error ? err.message : String(err) });
      }
      setProgress({ done: Math.min(i + EXEC_BATCH_SIZE, pendingOps.length), total: pendingOps.length });
    }
    // 执行后重扫刷新着色（演示模型已就地更新）
    try {
      const fresh = await mergeScan({ sessionId, base: merge.base, left: merge.left, right: merge.right });
      setRows(fresh.rows);
    } catch {
      // 保持旧行（真实引擎重扫失败不阻塞报告展示）
    }
    setReport(report);
    setPhase('report');
  };

  const exportReport = async (): Promise<void> => {
    if (!report) return;
    const text = exportMergeReportText({ target, base: merge.base, left: merge.left, right: merge.right }, report);
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // 剪贴板不可用时仅下载
    }
    const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'merge-report.txt';
    a.click();
    URL.revokeObjectURL(url);
    setNotice(S.mergeView.reportExported);
  };

  if (error) {
    return (
      <div className="aic-folder">
        <div className="aic-error-box">
          <b>{S.mergeView.openFail}</b>
          <pre>{error}</pre>
          <button onClick={() => void scan(true)}>{S.mergeView.refresh}</button>
        </div>
      </div>
    );
  }

  if (!rows) {
    return (
      <div className="aic-folder aic-merge">
        <div className="aic-sync-empty">{S.mergeView.scanRunning}</div>
      </div>
    );
  }

  return (
    <div className="aic-folder aic-merge">
      <div className="aic-folder-toolbar">
        <span className="aic-folder-paths" title={`${merge.base} | ${merge.left} | ${merge.right}`}>
          {merge.base} ▸ {merge.left} + {merge.right}
        </span>
        {decisionStats && (
          <span className="dim">
            {S.mergeView.statsTotal(rows.length)} · {S.mergeView.statsAuto(decisionStats.conflicts >= 0 ? rows.filter((r) => r.rowState === 'auto').length : 0)} ·{' '}
            <b className="merge-conflict-text">{S.mergeView.statsConflict(decisionStats.conflicts)}</b> · {S.mergeView.statsOrphan(decisionStats.orphans)}
          </span>
        )}
        <span className="sep" />
        <label>{S.mergeView.targetLabel}</label>
        <select
          className="aic-mini-select"
          value={target}
          disabled={phase === 'running'}
          onChange={(e) => {
            setTarget(e.target.value as DirMergeTarget);
            invalidatePlan();
          }}
        >
          <option value="right">{S.mergeView.targetRight}</option>
          <option value="left">{S.mergeView.targetLeft}</option>
          <option value="output">{S.mergeView.targetOutput}</option>
        </select>
        {target === 'output' && (
          <input
            className="filter-input"
            placeholder={S.mergeView.outputPathLabel}
            value={outputPath}
            onChange={(e) => {
              setOutputPath(e.target.value);
              invalidatePlan();
            }}
          />
        )}
        <span className="sep" />
        <button className="mini" disabled={phase === 'running'} onClick={() => setExpanded(new Set(rows.filter((r) => r.kind === 'dir').map((r) => r.relPath)))}>
          {S.mergeView.expandAll}
        </button>
        <button className="mini" disabled={phase === 'running'} onClick={() => setExpanded(new Set())}>
          {S.mergeView.collapseAll}
        </button>
        <button className="mini" disabled={phase === 'running' || Object.keys(decisions).length === 0} onClick={() => setDecisions({})}>
          {S.mergeView.clearDecisions}
        </button>
        {decisionStats && decisionStats.conflicts > 0 && (
          <span className="sel-chip">{S.mergeView.conflictsResolved(decisionStats.conflictsResolved, decisionStats.conflicts)}</span>
        )}
        <span className="sep" />
        {phase === 'tree' && (
          <button className="mini primary" onClick={() => void buildPlan()}>
            {S.mergeView.planTitle}
          </button>
        )}
      </div>

      {/* 阶段 2：计划确认 */}
      {phase === 'plan' && (
        <div className="aic-sync-plan">
          <div className="aic-sync-row">
            <b>{S.mergeView.planTitle}</b>
            <span className="dim">{S.mergeView.planDesc}</span>
          </div>
          <div className="aic-sync-stats">
            {planStats.takeLeft > 0 && <span className="chip l2r">{S.mergeView.opTakeLeft}× {planStats.takeLeft}</span>}
            {planStats.takeRight > 0 && <span className="chip r2l">{S.mergeView.opTakeRight}× {planStats.takeRight}</span>}
            {planStats.deleteTarget > 0 && <span className="chip warn">{S.mergeView.opDelete}× {planStats.deleteTarget}</span>}
            <span className="chip">{S.mergeView.opKeep}× {planStats.keep}</span>
          </div>
          {pendingOps.length === 0 ? (
            <div className="aic-sync-empty">{S.mergeView.planEmpty}</div>
          ) : (
            <div className="aic-sync-table">
              <div className="aic-sync-head">
                <span>{S.mergeView.planColOp}</span>
                <span>{S.syncView.planColPath}</span>
                <span>{S.syncView.planColReason}</span>
              </div>
              <div className="aic-sync-body">
                {preview.map((p) => (
                  <div key={p.path} className={`aic-sync-tr${p.warning ? ' warn' : ''}${p.actionable ? '' : ' skipped'}`}>
                    <span className={p.op === 'takeLeft' ? 'dir-l2r' : p.op === 'takeRight' ? 'dir-r2l' : ''}>
                      {OP_LABELS[p.op]}
                      {p.decision !== 'auto' && <i className="ov-badge">{DECISION_LABELS[p.decision]}</i>}
                    </span>
                    <span className="mono" title={p.path}>
                      {p.kind === 'dir' ? '📁 ' : ''}
                      {p.path}
                    </span>
                    <span className="dim reason">{p.reason}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
          <div className="aic-sync-actions">
            <button className="mini" onClick={() => setPhase('tree')}>
              {S.mergeView.reportBack}
            </button>
            <button className="mini primary" disabled={pendingOps.length === 0} onClick={() => void runMerge()}>
              {S.mergeView.planExecute}（{pendingOps.length}）
            </button>
          </div>
        </div>
      )}

      {/* 阶段 3：执行进度 */}
      {phase === 'running' && progress && (
        <div className="aic-sync-running">
          <span className="aic-progress">
            <span className="bar">
              <i style={{ width: `${progress.total === 0 ? 100 : Math.round((progress.done / progress.total) * 100)}%` }} />
            </span>
            {S.mergeView.running(progress.done, progress.total)}
          </span>
        </div>
      )}

      {/* 阶段 4：结果报告 */}
      {phase === 'report' && report && (
        <div className="aic-sync-report">
          <div className="aic-sync-row">
            <b>{S.mergeView.reportTitle}</b>
          </div>
          <div className="aic-sync-cards">
            <div className="card">
              <i>{report.copied}</i>
              {S.mergeView.reportCopied}
            </div>
            <div className="card">
              <i>{report.deleted}</i>
              {S.mergeView.reportDeleted}
            </div>
            <div className="card">
              <i>{report.skipped}</i>
              {S.mergeView.reportSkipped}
            </div>
            <div className={`card${report.failed > 0 ? ' bad' : ''}`}>
              <i>{report.failed}</i>
              {S.mergeView.reportFailed}
            </div>
          </div>
          <div className="aic-sync-actions">
            <button className="mini" onClick={() => setPhase('tree')}>
              {S.mergeView.reportBack}
            </button>
            <button className="mini" onClick={() => void exportReport()}>
              {S.mergeView.reportExport}
            </button>
          </div>
          <div className="aic-sync-table small">
            <div className="aic-sync-head">
              <span>{S.mergeView.planColOp}</span>
              <span>{S.syncView.planColPath}</span>
              <span>{S.mergeView.reportDetail}</span>
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

      {/* 三栏目录树（FR-DMRG-001/002） */}
      {phase === 'tree' && (
        <div className="aic-merge-table">
          <div className="aic-merge-head">
            <span>{S.folder.nameCol}</span>
            <span className="c">{S.mergeView.colBase}</span>
            <span className="c">{S.mergeView.colLeft}</span>
            <span className="c">{S.mergeView.colRight}</span>
            <span className="c">{S.mergeView.colDecision}</span>
          </div>
          <div className="aic-merge-scroll">
            {visibleRows.map((row) => (
              <MergeRowLine
                key={row.relPath}
                row={row}
                expanded={expanded.has(row.relPath)}
                decision={effectiveDecisionLocal(row)}
                onToggle={() => toggleDir(row)}
                onDecision={(d) => setDecision(row, d)}
                onOpenConflict={() => {
                  if (row.conflictLeaf) setTextMergeRow(row);
                }}
              />
            ))}
          </div>
        </div>
      )}

      {/* 双击冲突文件行：文本合并视图占位（P10 后接真三向合并 UI） */}
      {textMergeRow && (
        <div className="aic-modal-mask" onClick={() => setTextMergeRow(null)}>
          <div className="aic-modal" onClick={(e) => e.stopPropagation()}>
            <div className="aic-modal-title">{S.mergeView.textMergeTitle} — {textMergeRow.relPath}</div>
            <div className="aic-modal-body">{S.mergeView.textMergePlaceholder}</div>
            <div className="aic-modal-actions">
              <button className="mini" onClick={() => setTextMergeRow(null)}>
                {S.mergeView.textMergeClose}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );

  function effectiveDecisionLocal(row: DirMergeRow): DirMergeDecision {
    // 与 mergeRows.effectiveDecision 同口径（此处经闭包取 decisions）
    let cur: string | null = row.relPath;
    while (cur) {
      const d = decisions[cur];
      if (d) return d;
      cur = cur.includes('/') ? cur.slice(0, cur.lastIndexOf('/')) : null;
    }
    return 'auto';
  }
}

/** 三栏行渲染（名称缩进 + 类别 + 三侧信息 + 决策按钮组） */
function MergeRowLine({
  row,
  expanded,
  decision,
  onToggle,
  onDecision,
  onOpenConflict,
}: {
  row: DirMergeRow;
  expanded: boolean;
  decision: DirMergeDecision;
  onToggle(): void;
  onDecision(d: DirMergeDecision): void;
  onOpenConflict(): void;
}) {
  const isDir = row.kind === 'dir';
  const stateClass = mergeStateClass(row.rowState);
  // 目录行 = 聚合状态文案；叶行 = 叶级分类文案
  const catLabel = isDir ? MERGE_STATE_LABELS[row.rowState] : MERGE_CATEGORY_LABELS[row.category];
  const decButtons: DirMergeDecision[] = decision === 'auto' ? ['takeLeft', 'takeRight', 'both', 'skip'] : ['takeLeft', 'takeRight', 'both', 'skip', 'auto'];
  return (
    <div className={`aic-merge-tr ${stateClass}`} onDoubleClick={() => !isDir && onOpenConflict()}>
      <span className="name" style={{ paddingLeft: 8 + (row.depth - 1) * 16 }} title={`${row.relPath} · ${catLabel}${row.conflictLeaf ? ' · 双击打开文本合并占位' : ''}`}>
        <span
          className={`aic-dir-caret${isDir ? '' : ' empty'}`}
          onClick={(e) => {
            e.stopPropagation();
            if (isDir) onToggle();
          }}
        >
          {isDir ? (expanded ? '▾' : '▸') : ''}
        </span>
        {row.name}
        <i className="cat-badge">{catLabel}</i>
        {decision !== 'auto' && <i className="dec-badge">{DECISION_LABELS[decision]}</i>}
      </span>
      <span className="side c mono">{row.base ? `${formatSize(row.base.size)} · ${formatTime(row.base.mtimeMs)}` : '—'}</span>
      <span className="side c mono">{row.left ? `${formatSize(row.left.size)} · ${formatTime(row.left.mtimeMs)}` : '—'}</span>
      <span className="side c mono">{row.right ? `${formatSize(row.right.size)} · ${formatTime(row.right.mtimeMs)}` : '—'}</span>
      <span className="c decisions">
        {decButtons.map((d) => (
          <button key={d} className={`dec-btn${decision === d ? ' on' : ''}`} onClick={() => onDecision(d)} title={DECISION_LABELS[d]}>
            {DECISION_LABELS[d]}
          </button>
        ))}
      </span>
    </div>
  );
}
