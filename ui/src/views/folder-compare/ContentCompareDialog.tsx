/**
 * 手动内容比较结果对话框（R3-16，FR-DIR-012「操作 > 比较内容」）。
 * 展示每行的五态结论与比较后的行状态；方法/范围来自调用方（视图层）。
 */

import type { ContentCompareMethod, DirContentCompareRow } from '../../protocol/types';
import { S } from '../../strings';
import { F } from './localStrings';
import { RESULT_LABELS, summarizeContentCompare } from './contentCompare';
import { statusVisual } from './statusDisplay';

export interface ContentCompareDialogState {
  method: ContentCompareMethod;
  /** 范围说明（选定 N 项 / 全部已配对文件） */
  scopeLabel: string;
  /** null = 请求进行中 */
  results: readonly DirContentCompareRow[] | null;
  error: string | null;
  onClose(): void;
}

export const METHOD_LABELS: Record<ContentCompareMethod, string> = {
  crc: F.contentCompareMethodCrc,
  binary: F.contentCompareMethodBinary,
  rulesBased: F.contentCompareMethodRules,
};

const cell: React.CSSProperties = {
  padding: '3px 10px',
  borderTop: '1px solid var(--aic-border)',
  whiteSpace: 'nowrap',
};

export function ContentCompareDialog({ state }: { state: ContentCompareDialogState }) {
  const results = state.results;
  const summary = results ? summarizeContentCompare(results) : null;
  return (
    <div className="aic-modal-mask">
      <div
        className="aic-modal aic-dialog"
        role="dialog"
        aria-label={F.contentCompareTitle}
        style={{ width: 620, maxWidth: '94vw' }}
      >
        <h3 style={{ margin: '0 0 4px' }}>{F.contentCompareTitle}</h3>
        <p className="dim" style={{ margin: '0 0 8px', fontSize: 12 }}>
          {F.contentCompareMethodLabel}：{METHOD_LABELS[state.method]} · {state.scopeLabel}
        </p>
        {state.error && <div className="aic-plan-warning">{state.error}</div>}
        {results === null && !state.error && (
          <div className="aic-progress">{F.contentCompareRunning}</div>
        )}
        {summary && results && (
          <>
            <div className="aic-plan-warning" style={{ marginTop: 4 }}>
              {F.contentCompareDone(summary.total)} — {summary.message}
            </div>
            <div style={{ maxHeight: '46vh', overflow: 'auto', marginTop: 8 }}>
              <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 12 }}>
                <thead>
                  <tr style={{ textAlign: 'left', color: 'var(--aic-text-dim)' }}>
                    <th style={{ padding: '2px 10px' }}>{F.contentCompareColId}</th>
                    <th style={{ padding: '2px 10px' }}>{F.contentCompareColResult}</th>
                    <th style={{ padding: '2px 10px' }}>{F.contentCompareColStatus}</th>
                  </tr>
                </thead>
                <tbody>
                  {results.map((r) => (
                    <tr key={r.id}>
                      <td style={cell}>{r.id}</td>
                      <td style={cell}>{RESULT_LABELS[r.result]}</td>
                      <td style={cell} className={`aic-status-${statusVisual(r.status).tone}`}>
                        {statusVisual(r.status).label}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
        <div className="aic-dialog-actions">
          <button onClick={state.onClose}>{S.folder.opsClose}</button>
        </div>
      </div>
    </div>
  );
}
