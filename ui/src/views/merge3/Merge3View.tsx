import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  getEngineCapabilities,
  merge3Decide,
  merge3OpenIdempotent,
  merge3Output,
  merge3Sections,
  saveMerge3,
} from '../../ipc/engineProcess';
import type {
  LineRange,
  MergeChunkDto,
  MergeChunkOrigin,
  MergeConflictKind,
  MergeDecisionDto,
  MergeDecisionKind,
  MergeMarkerStyle,
  MergeOpenResult,
  MergeOutputMode,
  MergeOutputResult,
} from '../../protocol/types';
import type { Tab } from '../../shell/store';
import { useShell } from '../../shell/store';
import { S } from '../../strings';
import { runMergeSave } from './mergeSave';

/**
 * 三向文本合并视图（队列 3；消费 P7 的 text/merge3/open|sections|decide|output）。
 *
 * BC 忠实度基准 = docs/01 FR-TMRG-001~007：
 * - 001 三源窗格（左/右/可选中间=公共祖先，均不可编辑）+ 下方输出窗格；
 *   中间窗格可临时隐藏。
 * - 002 颜色语义：仅左改=青、仅右改=洋红、左右不同更改（冲突）=红、
 *   未更改=灰；左右相同更改不冲突、自动合并（绿）；输出被手动编辑=黄。
 *   显示过滤器五项含组合语义（"只显示左侧更改"含相同更改/左侧更改/冲突）。
 * - 004 采用命令族：取左/取中/取右/先左后右/先右后左/手动编辑，
 *   支持对所有未决冲突批量提交（decide 批量）。
 * - 005/006 输出编辑（manual 决策）与输出预览/复制/导出（plain 与 marker 两模式）。
 * - 007 合并统计（块/冲突/未决/已决策/rev/三方行数）。
 */

/** 显示过滤器（FR-TMRG-002 合并专用五项） */
export type Merge3Filter = 'all' | 'changes' | 'conflicts' | 'leftChanges' | 'rightChanges';

export const MERGE3_FILTERS: readonly Merge3Filter[] = [
  'all',
  'changes',
  'conflicts',
  'leftChanges',
  'rightChanges',
];

/** 六动作采用族中的五个直接动作（manual 需附带编辑文本，单独入口） */
export const MERGE3_TAKE_ACTIONS: readonly MergeDecisionKind[] = [
  'left',
  'right',
  'base',
  'leftThenRight',
  'rightThenLeft',
];

/**
 * 块来源 → 着色类（FR-TMRG-002 / §2.3）。
 * manual 决策块优先着黄：FR-TMRG-005「输出中被手动编辑的部分黄色」。
 */
export function chunkCls(chunk: MergeChunkDto): string {
  if (chunk.decision === 'manual') return 'm3-manual';
  switch (chunk.origin) {
    case 'leftOnly':
      return 'm3-left';
    case 'rightOnly':
      return 'm3-right';
    case 'conflict':
      return 'm3-conflict';
    case 'bothChanged':
      return 'm3-both';
    default:
      return 'm3-stable';
  }
}

export function originLabel(origin: MergeChunkOrigin): string {
  switch (origin) {
    case 'leftOnly':
      return S.merge3.originLeftOnly;
    case 'rightOnly':
      return S.merge3.originRightOnly;
    case 'conflict':
      return S.merge3.originConflict;
    case 'bothChanged':
      return S.merge3.originBothChanged;
    default:
      return S.merge3.originStable;
  }
}

/**
 * 显示过滤器（FR-TMRG-002 组合语义）：
 * 「只显示左侧更改」实际显示相同更改 + 左侧更改 + 左右不同更改（冲突）；
 * 「只显示右侧更改」对称。冲突与相同更改在两侧筛选中都保留。
 */
export function filterChunks(chunks: MergeChunkDto[], filter: Merge3Filter): MergeChunkDto[] {
  switch (filter) {
    case 'all':
      return chunks;
    case 'changes':
      return chunks.filter((c) => c.origin !== 'stable');
    case 'conflicts':
      return chunks.filter((c) => c.origin === 'conflict');
    case 'leftChanges':
      return chunks.filter(
        (c) => c.origin === 'leftOnly' || c.origin === 'bothChanged' || c.origin === 'conflict'
      );
    case 'rightChanges':
      return chunks.filter(
        (c) => c.origin === 'rightOnly' || c.origin === 'bothChanged' || c.origin === 'conflict'
      );
  }
}

/** 块决策状态文案（按优先级：手动编辑 > 已采用 > 自动采用 > 冲突未决） */
export function stateLabel(chunk: MergeChunkDto): string {
  if (chunk.decision === 'manual') return S.merge3.stateManual;
  if (chunk.decided) return S.merge3.stateDecided;
  if (chunk.autoResolved) return S.merge3.stateAutoResolved;
  if (chunk.unresolved) return S.merge3.stateUnresolved;
  return '';
}

export function conflictKindLabel(kind: MergeConflictKind | null): string {
  switch (kind) {
    case 'modifyModify':
      return S.merge3.conflictModifyModify;
    case 'deleteModify':
      return S.merge3.conflictDeleteModify;
    case 'addAdd':
      return S.merge3.conflictAddAdd;
    default:
      return '';
  }
}

export function decisionLabel(action: MergeDecisionKind): string {
  switch (action) {
    case 'left':
      return S.merge3.takeLeft;
    case 'right':
      return S.merge3.takeRight;
    case 'base':
      return S.merge3.takeBase;
    case 'leftThenRight':
      return S.merge3.takeLeftThenRight;
    case 'rightThenLeft':
      return S.merge3.takeRightThenLeft;
    case 'manual':
      return S.merge3.manual;
  }
}

/** 半开行区间 → BC 风格 1 起行号（空区间 = 该侧无内容） */
export function rangeText(r: LineRange): string {
  if (r.end <= r.start) return '—';
  return r.end === r.start + 1 ? String(r.start + 1) : `${r.start + 1}-${r.end}`;
}

/**
 * 取一侧行内容。真实引擎对空区间会省略行内容字段（如右侧插入块没有
 * baseLines/leftLines，serde skip_serializing_if），而 ts-rs 生成类型仍
 * 声明为必填数组 —— UI 必须按空数组兜底，否则视图渲染会崩。
 */
export function paneLines(chunk: MergeChunkDto, side: 'base' | 'left' | 'right'): string[] {
  const lines = side === 'base' ? chunk.baseLines : side === 'left' ? chunk.leftLines : chunk.rightLines;
  return lines ?? [];
}

/** 合并统计（FR-TMRG-007；测试锚点） */
export function formatMergeStats(s: {
  chunkCount: number;
  conflictCount: number;
  unresolved: number;
  decided: number;
}): string {
  return `${s.chunkCount} ${S.merge3.chunks} / ${s.conflictCount} ${S.merge3.conflicts} / ${s.unresolved} ${S.merge3.unresolved} / ${s.decided} ${S.merge3.decided}`;
}

export function filterLabel(f: Merge3Filter): string {
  switch (f) {
    case 'all':
      return S.merge3.filterAll;
    case 'changes':
      return S.merge3.filterChanges;
    case 'conflicts':
      return S.merge3.filterConflicts;
    case 'leftChanges':
      return S.merge3.filterLeftChanges;
    case 'rightChanges':
      return S.merge3.filterRightChanges;
  }
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** 单窗格：按块序列渲染三方行内容（只读；FR-TMRG-001 源窗格不可编辑） */
function ChunkPane({
  side,
  label,
  chunks,
}: {
  side: 'base' | 'left' | 'right';
  label: string;
  chunks: MergeChunkDto[];
}) {
  return (
    <div className="aic-m3-pane" data-side={side}>
      <div className="aic-m3-pane-head">{label}</div>
      <div className="aic-m3-pane-body">
        {chunks.map((c) => {
          const range = side === 'base' ? c.base : side === 'left' ? c.left : c.right;
          const lines = paneLines(c, side);
          const state = stateLabel(c);
          const kind = conflictKindLabel(c.conflictKind);
          return (
            <div
              key={c.id}
              className={`aic-m3-chunk ${chunkCls(c)}`}
              data-chunk-id={c.id}
              data-origin={c.origin}
            >
              <div className="aic-m3-chunk-head">
                <span className="m3-id">#{c.id}</span>
                <span className="m3-range">{rangeText(range)}</span>
                <span className="m3-origin">{originLabel(c.origin)}</span>
                {kind && <span className="m3-kind">{kind}</span>}
                {c.conflictUnimportant && <span className="m3-kind">{S.merge3.stateUnimportant}</span>}
              </div>
              {lines.length === 0 ? (
                <div className="aic-m3-empty">—</div>
              ) : (
                lines.map((line, i) => (
                  <div className="aic-m3-line" key={i}>
                    <span className="aic-m3-ln">{range.start + i + 1}</span>
                    <span className="aic-m3-text">{line === '' ? ' ' : line}</span>
                  </div>
                ))
              )}
              {state && <div className="aic-m3-flag">{state}</div>}
            </div>
          );
        })}
        {chunks.length === 0 && <div className="aic-m3-empty">{S.merge3.loading}</div>}
      </div>
    </div>
  );
}

/** 单页块数（text/merge3/sections 分批窗口） */
const SECTION_PAGE = 200;

type Merge3Tab = Tab & {
  kind: 'merge3';
  merge3: { base: string; left: string; right: string; sessionId: string; outputPath?: string };
};

export function Merge3View({ tab }: { tab: Merge3Tab }) {
  const { sessionId, base, left, right } = tab.merge3;
  const [opened, setOpened] = useState<MergeOpenResult | null>(null);
  const [chunks, setChunks] = useState<MergeChunkDto[]>([]);
  const [output, setOutput] = useState<MergeOutputResult | null>(null);
  const [rev, setRev] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState<Merge3Filter>('all');
  const [showBase, setShowBase] = useState(true);
  const [mode, setMode] = useState<MergeOutputMode>('plain');
  const [markerStyle, setMarkerStyle] = useState<MergeMarkerStyle>('merge');
  const [manualId, setManualId] = useState<number | null>(null);
  const [manualText, setManualText] = useState('');
  /** 保存前备份（FR-TMRG-006；关闭 = BC 的 /nobackups） */
  const [backup, setBackup] = useState(true);

  const loadSections = useCallback(async (): Promise<MergeChunkDto[]> => {
    const all: MergeChunkDto[] = [];
    let offset = 0;
    let lastRev = 0;
    // 防御性上限：协议异常（hasMore 永不置 false）时避免死循环
    for (let guard = 0; guard < 1000; guard++) {
      const r = await merge3Sections({ sessionId, offset, count: SECTION_PAGE });
      all.push(...r.chunks);
      lastRev = Number(r.rev);
      offset += r.chunks.length;
      if (!r.hasMore || r.chunks.length === 0) break;
    }
    setRev(lastRev);
    return all;
  }, [sessionId]);

  const refreshOutput = useCallback(async () => {
    const r = await merge3Output({ sessionId, mode, markerStyle });
    setOutput(r);
  }, [sessionId, mode, markerStyle]);

  // 打开会话 + 拉全块清单；只在会话句柄变化时执行一次。
  // 幂等封装处理 411（React StrictMode 双挂载 / 标签重挂载）：先 close 再 open。
  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const r = await merge3OpenIdempotent({ sessionId, base, left, right, options: null });
        if (disposed) return;
        setOpened(r);
        const all = await loadSections();
        if (!disposed) setChunks(all);
      } catch (err) {
        if (!disposed) setError(messageOf(err));
      }
    })();
    return () => {
      disposed = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // 输出模式/标记风格切换、决策提交后重取合成结果
  useEffect(() => {
    if (!opened) return;
    void refreshOutput().catch((err: unknown) => setNotice(messageOf(err)));
  }, [opened, refreshOutput]);

  /** decide 批量提交（幂等）：提交后重拉块清单与输出 */
  const applyDecisions = useCallback(
    async (decisions: MergeDecisionDto[]) => {
      if (decisions.length === 0) return;
      setBusy(true);
      try {
        const r = await merge3Decide({ sessionId, decisions });
        setRev(Number(r.rev));
        setChunks(await loadSections());
        await refreshOutput();
        setManualId(null);
      } catch (err) {
        setNotice(`${S.merge3.decideFailed}: ${messageOf(err)}`);
      } finally {
        setBusy(false);
      }
    },
    [sessionId, loadSections, refreshOutput]
  );

  const decideAll = (action: MergeDecisionKind) => {
    const ids = chunks.filter((c) => c.unresolved).map((c) => c.id);
    void applyDecisions(ids.map((chunkId) => ({ chunkId, action, text: null })));
  };

  const visible = useMemo(() => filterChunks(chunks, filter), [chunks, filter]);
  const conflicts = useMemo(() => visible.filter((c) => c.origin === 'conflict'), [visible]);

  const copyOutput = async () => {
    try {
      await navigator.clipboard.writeText(output?.text ?? '');
      setNotice(S.merge3.copied);
    } catch {
      setNotice(S.merge3.copyFailed);
    }
  };

  /**
   * 保存输出到"合并到"目标（R3-20，FR-TMRG-006）。
   * 引擎按会话决策现场合成后直接落盘，不回传预览文本：非活动标签/大输出同样可保存。
   */
  const doSaveOutput = useCallback(
    async (path: string) => {
      setBusy(true);
      try {
        const out = await runMergeSave(
          sessionId,
          path,
          { mode, markerStyle, backup },
          {
            save: saveMerge3,
            confirmOverwrite: (target) =>
              useShell.getState().askConfirm({
                title: S.merge3.overwriteTitle,
                message: `${S.merge3.overwriteBody}：${target}${
                  backup ? S.merge3.overwriteBackupSuffix : S.merge3.overwriteNoBackupSuffix
                }`,
                confirmLabel: S.merge3.saveOutput,
              }),
          },
        );
        if (out.cancelled) {
          setNotice(S.merge3.saveCancelled);
          return;
        }
        if (out.unsupported) {
          setNotice(S.merge3.saveUnsupported);
          return;
        }
        if (out.error !== null) {
          setNotice(`${S.merge3.saveFail}: ${out.error}`);
          return;
        }
        const res = out.result;
        if (!res) return;
        useShell.getState().setMerge3OutputPath(tab.id, res.path);
        const backupNote =
          res.backupPath === undefined ? '' : `（${S.merge3.savedBackupSuffix} ${res.backupPath}）`;
        const conflictNote = res.hasConflicts
          ? `（${S.merge3.saveConflictHint}：${res.unresolvedChunks.join('、')}）`
          : '';
        setNotice(`${S.merge3.saved}：${res.path}${backupNote}${conflictNote}`);
      } finally {
        setBusy(false);
      }
    },
    [sessionId, mode, markerStyle, backup, tab.id],
  );

  const saveOutput = () => {
    useShell.getState().openPrompt({
      title: S.merge3.saveTitle,
      initialValue: tab.merge3.outputPath ?? '',
      onSubmit: (value) => {
        if (value === null) return;
        const path = value.trim();
        if (!path) return;
        void doSaveOutput(path);
      },
    });
  };

  if (error) {
    return <div className="aic-m3-error">{S.merge3.loadFail}: {error}</div>;
  }

  return (
    <div className="aic-m3">
      <div className="aic-m3-toolbar">
        <span className="aic-m3-title">
          {S.merge3.title}: {tab.title.replace(`${S.tab.merge3TitleSuffix}：`, '')}
        </span>
        {!getEngineCapabilities()?.threeWayMerge && (
          <span className="aic-m3-cap">（{S.merge3.capNote}）</span>
        )}
        <label>
          {S.merge3.filter}
          <select value={filter} onChange={(e) => setFilter(e.target.value as Merge3Filter)}>
            {MERGE3_FILTERS.map((f) => (
              <option key={f} value={f}>{filterLabel(f)}</option>
            ))}
          </select>
        </label>
        <label>
          <input type="checkbox" checked={showBase} onChange={(e) => setShowBase(e.target.checked)} />
          {showBase ? S.merge3.hideBase : S.merge3.showBase}
        </label>
        <button disabled={busy} onClick={() => decideAll('left')}>{S.merge3.allTakeLeft}</button>
        <button disabled={busy} onClick={() => decideAll('right')}>{S.merge3.allTakeRight}</button>
      </div>

      <div className="aic-m3-stats">
        {formatMergeStats({
          chunkCount: opened?.chunkCount ?? 0,
          conflictCount: opened?.conflictCount ?? 0,
          unresolved: output?.unresolvedChunks.length ?? 0,
          decided: output?.decidedCount ?? 0,
        })}
        <span>{S.merge3.rev}: {rev}</span>
        <span>
          {S.merge3.lineCounts}: {opened ? `${opened.baseLineCount}/${opened.leftLineCount}/${opened.rightLineCount}` : '—'}
        </span>
      </div>

      <div className="aic-m3-legend">
        <span>{S.merge3.legend}:</span>
        <span className="m3-stable">■ {S.merge3.originStable}</span>
        <span className="m3-left">■ {S.merge3.originLeftOnly}</span>
        <span className="m3-right">■ {S.merge3.originRightOnly}</span>
        <span className="m3-both">■ {S.merge3.originBothChanged}</span>
        <span className="m3-conflict">■ {S.merge3.originConflict}</span>
        <span className="m3-manual">■ {S.merge3.stateManual}</span>
      </div>

      {notice && <div className="aic-m3-notice" onClick={() => setNotice(null)}>{notice}</div>}

      <div className={`aic-m3-panes${showBase ? '' : ' no-base'}`}>
        {showBase && <ChunkPane side="base" label={S.merge3.basePane} chunks={visible} />}
        <ChunkPane side="left" label={S.merge3.leftPane} chunks={visible} />
        <ChunkPane side="right" label={S.merge3.rightPane} chunks={visible} />
      </div>

      <div className="aic-m3-conflicts">
        <div className="aic-m3-section-head">{S.merge3.originConflict}（{conflicts.length}）</div>
        {conflicts.map((c) => (
          <div key={c.id} className={`aic-m3-decision ${chunkCls(c)}`} data-chunk-id={c.id}>
            <span className="m3-id">#{c.id}</span>
            <span className="m3-kind">{conflictKindLabel(c.conflictKind)}</span>
            {c.conflictUnimportant && <span className="m3-kind">{S.merge3.stateUnimportant}</span>}
            <span className="m3-state">{stateLabel(c)}</span>
            <div className="aic-m3-actions">
              {MERGE3_TAKE_ACTIONS.map((a) => (
                <button
                  key={a}
                  disabled={busy}
                  onClick={() => void applyDecisions([{ chunkId: c.id, action: a, text: null }])}
                >
                  {decisionLabel(a)}
                </button>
              ))}
              <button
                disabled={busy}
                onClick={() => {
                  setManualId(c.id);
                  setManualText((c.outputLines ?? []).join('\n'));
                }}
              >
                {S.merge3.manual}
              </button>
            </div>
            {manualId === c.id && (
              <div className="aic-m3-manual">
                <textarea
                  value={manualText}
                  title={S.merge3.manualHint}
                  onChange={(e) => setManualText(e.target.value)}
                />
                <button
                  disabled={busy}
                  onClick={() => void applyDecisions([{ chunkId: c.id, action: 'manual', text: manualText }])}
                >
                  {S.merge3.manualApply}
                </button>
                <button onClick={() => setManualId(null)}>{S.merge3.manualCancel}</button>
              </div>
            )}
          </div>
        ))}
      </div>

      <div className="aic-m3-output">
        <div className="aic-m3-section-head">
          {S.merge3.outputTitle}
          <select value={mode} onChange={(e) => setMode(e.target.value as MergeOutputMode)}>
            <option value="plain">{S.merge3.outputPlain}</option>
            <option value="marker">{S.merge3.outputMarker}</option>
          </select>
          {mode === 'marker' && (
            <select value={markerStyle} onChange={(e) => setMarkerStyle(e.target.value as MergeMarkerStyle)}>
              <option value="merge">{S.merge3.markerMerge}</option>
              <option value="diff3">{S.merge3.markerDiff3}</option>
            </select>
          )}
          <button onClick={() => void copyOutput()}>{S.merge3.copyOutput}</button>
          <button disabled={busy} onClick={saveOutput}>
            {S.merge3.saveOutput}
          </button>
          <label>
            <input type="checkbox" checked={backup} onChange={(e) => setBackup(e.target.checked)} />
            {S.merge3.backup}
          </label>
          <button disabled={busy} onClick={() => void refreshOutput()}>{S.merge3.refresh}</button>
        </div>
        {output?.hasConflicts && <div className="aic-m3-warn">{S.merge3.unresolvedHint}</div>}
        <pre className="aic-m3-output-text">{output?.text ?? S.merge3.loading}</pre>
      </div>
    </div>
  );
}
