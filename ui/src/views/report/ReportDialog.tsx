import { useEffect, useState } from 'react';
import { generateReport, type ReportParams } from '../../ipc/engineProcess';
import type {
  ReportColorMode,
  ReportDirDisplayFilter,
  ReportDisplayFilter,
  ReportLayout,
  ReportPatchStyle,
  ReportWrapMode,
} from '../../protocol/types';
import { useShell } from '../../shell/store';
import { S } from '../../strings';
import {
  LAYOUTS_BY_KIND,
  REPORT_CHUNK,
  REPORT_LAYOUT_LABELS,
  REPORT_TARGET_LABELS,
  buildReportParams,
  defaultReportForm,
  deliverReport,
  isHtmlTarget,
  wrapReportText,
  type ReportForm,
  type ReportKind,
  type ReportTarget,
} from './reportModel';

const DISPLAY_OPTIONS: Array<[ReportDisplayFilter, string]> = [
  ['all', S.report.displayAll],
  ['mismatches', S.report.displayMismatches],
  ['context', S.report.displayContext],
  ['matches', S.report.displayMatches],
];

const DIR_DISPLAY_OPTIONS: ReportDirDisplayFilter[] = [
  'all', 'mismatches', 'noOrphans', 'mismatchesNoOrphans', 'orphans', 'matches',
  'leftNewer', 'rightNewer', 'leftNewerOrphans', 'rightNewerOrphans', 'leftOrphans', 'rightOrphans',
];

const COLUMN_KEYS = [
  'size', 'timestamp', 'attributes', 'crc', 'vcs', 'revision', 'version', 'owner', 'group',
] as const;

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** FR-RPT-001/002 报告对话框：布局 + 布局级选项 + 输出目标 + 预览 */
export function ReportDialog() {
  const dialog = useShell((st) => st.reportDialog);
  const close = useShell((st) => st.closeReportDialog);
  const notice = useShell((st) => st.setNotice);
  const [form, setForm] = useState<ReportForm>(() => defaultReportForm('text'));
  const [target, setTarget] = useState<ReportTarget>('preview');
  const [content, setContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!dialog) return;
    setForm(defaultReportForm(dialog.kind as ReportKind));
    setContent('');
    setError(null);
    setTarget('preview');
  }, [dialog]);

  if (!dialog) return null;
  const kind = dialog.kind as ReportKind;
  const layouts = LAYOUTS_BY_KIND[kind];
  const patch = (p: Partial<ReportForm>) => setForm((f) => ({ ...f, ...p }));
  const isDir = kind === 'folder' || kind === 'sync' || kind === 'merge';

  const generate = async () => {
    setBusy(true);
    setError(null);
    try {
      const params: ReportParams = buildReportParams(form, dialog.sessionId, kind);
      let text = '';
      let offset = 0;
      // 分块流：按引擎返回的 Unicode 标量数推进 offset，拼回全文
      for (let guard = 0; guard < 1000; guard++) {
        const r = await generateReport({ ...params, offset, limit: REPORT_CHUNK });
        text += r.content;
        offset += Array.from(r.content).length;
        if (!r.hasMore || r.content.length === 0) break;
      }
      const wrapped = wrapReportText(text, form.wrap, form.wrapColumns);
      setContent(wrapped);
      await deliverReport(wrapped, target, form.layout, notice);
    } catch (err) {
      setError(`${S.report.failed}: ${messageOf(err)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="aic-rpt-backdrop" onClick={() => close()}>
      <div className="aic-rpt-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="aic-rpt-head">
          <b>{S.report.title}</b>
          <span className="aic-rpt-sub">{dialog.sessionId}</span>
          <button onClick={() => close()}>{S.report.close}</button>
        </div>
        <div className="aic-rpt-body">
          <div className="aic-rpt-form">
            <label>
              {S.report.layout}
              <select value={form.layout} onChange={(e) => patch({ layout: e.target.value as ReportLayout })}>
                {layouts.map((l) => <option key={l} value={l}>{REPORT_LAYOUT_LABELS[l]}</option>)}
              </select>
            </label>
            <label>
              {S.report.reportTitle}
              <input
                value={form.title}
                placeholder={S.report.titlePlaceholder}
                onChange={(e) => patch({ title: e.target.value })}
              />
            </label>
            <label>
              {S.report.target}
              <select value={target} onChange={(e) => setTarget(e.target.value as ReportTarget)}>
                {(Object.keys(REPORT_TARGET_LABELS) as ReportTarget[]).map((t) => (
                  <option key={t} value={t}>{REPORT_TARGET_LABELS[t]}</option>
                ))}
              </select>
            </label>

            <div className="aic-rpt-section">{S.report.options}</div>
            <label>
              {S.report.display}
              <select value={form.display} onChange={(e) => patch({ display: e.target.value as ReportDisplayFilter })}>
                {DISPLAY_OPTIONS.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
              </select>
            </label>
            {isDir && (
              <label>
                {S.report.dirDisplay}
                <select value={form.dirDisplay} onChange={(e) => patch({ dirDisplay: e.target.value as ReportDirDisplayFilter })}>
                  {DIR_DISPLAY_OPTIONS.map((v) => <option key={v} value={v}>{v}</option>)}
                </select>
              </label>
            )}
            <label>
              {S.report.patchStyle}
              <select value={form.patchStyle} onChange={(e) => patch({ patchStyle: e.target.value as ReportPatchStyle })}>
                <option value="normal">{S.report.patchNormal}</option>
                <option value="context">{S.report.patchContext}</option>
                <option value="unified">{S.report.patchUnified}</option>
              </select>
            </label>
            <label>
              {S.report.delimiter}
              <select value={form.delimiter} onChange={(e) => patch({ delimiter: e.target.value as 'csv' | 'tsv' })}>
                <option value="csv">CSV</option>
                <option value="tsv">TSV</option>
              </select>
            </label>
            <label>
              {S.report.color}
              <select value={form.color} onChange={(e) => patch({ color: e.target.value as ReportColorMode })}>
                <option value="color">{S.report.colorColor}</option>
                <option value="mono">{S.report.colorMono}</option>
              </select>
            </label>
            <label>
              {S.report.wrap}
              <select value={form.wrap} onChange={(e) => patch({ wrap: e.target.value as ReportWrapMode })}>
                <option value="none">{S.report.wrapNone}</option>
                <option value="character">{S.report.wrapCharacter}</option>
                <option value="word">{S.report.wrapWord}</option>
              </select>
            </label>
            <label>
              {S.report.wrapColumns}
              <input type="number" min={1} value={form.wrapColumns} onChange={(e) => patch({ wrapColumns: Number(e.target.value) })} />
            </label>
            <label>
              {S.report.contextLines}
              <input type="number" min={0} value={form.contextLines} onChange={(e) => patch({ contextLines: Number(e.target.value) })} />
            </label>
            <label className="aic-rpt-check">
              <input type="checkbox" checked={form.ignoreUnimportant} onChange={(e) => patch({ ignoreUnimportant: e.target.checked })} />
              {S.report.ignoreUnimportant}
            </label>
            <label className="aic-rpt-check">
              <input type="checkbox" checked={form.lineNumbers} onChange={(e) => patch({ lineNumbers: e.target.checked })} />
              {S.report.lineNumbers}
            </label>
            <label className="aic-rpt-check">
              <input type="checkbox" checked={form.includeFileLinks} onChange={(e) => patch({ includeFileLinks: e.target.checked })} />
              {S.report.includeFileLinks}
            </label>
            {isDir && (
              <div className="aic-rpt-columns">
                <span>{S.report.columns}</span>
                {COLUMN_KEYS.map((k) => (
                  <label key={k} className="aic-rpt-check">
                    <input
                      type="checkbox"
                      checked={form.columns[k]}
                      onChange={(e) => patch({ columns: { ...form.columns, [k]: e.target.checked } })}
                    />
                    {k}
                  </label>
                ))}
              </div>
            )}
          </div>

          <div className="aic-rpt-preview">
            <div className="aic-rpt-preview-head">
              {S.report.preview}
              {content && <span>{Array.from(content).length} {S.report.chars}</span>}
              <button disabled={busy} onClick={() => void generate()}>
                {busy ? S.report.busy : S.report.generate}
              </button>
            </div>
            {error && <div className="aic-rpt-error">{error}</div>}
            <pre>{content || S.report.busy}</pre>
            {content && isHtmlTarget(target, form.layout) && <div className="aic-rpt-hint">HTML 输出</div>}
          </div>
        </div>
      </div>
    </div>
  );
}
