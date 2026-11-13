/**
 * 批量重命名对话框（FR-DIR-064 多文件重命名，P6b）：
 * - DOS 掩码模式 / 正则模板模式切换（renameTemplate.ts 纯函数求值）；
 * - 实时预览表（旧名 → 新名 + 状态列：将重命名/不变/未命中保留/冲突原因）；
 * - 冲突拦截：批次内重名、与现有行撞名、非法字符——存在任一冲突时执行禁用；
 * - onOk 交付可执行清单（renames，已按「子先父后」排序），由视图层转
 *   dirOps rename ops 走既有确认→dir/execute 批次链。
 */

import { useMemo, useState } from 'react';
import type { DirRow } from '../../protocol/types';
import { S } from '../../strings';
import { F } from './localStrings';
import { planRenames, type RenameMode, type RenamePlanItem } from './renameTemplate';

const PREVIEW_LIMIT = 200;

export interface BatchRenameDialogState {
  rows: readonly DirRow[];
  allRows: readonly DirRow[];
  onOk(renames: readonly { row: DirRow; newName: string }[]): void;
  onCancel(): void;
}

function stateNote(item: RenamePlanItem): { text: string; cls: string } {
  if (item.state === 'invalid') return { text: item.reason ?? '非法', cls: 'bad' };
  if (item.conflict === 'duplicate' || item.conflict === 'exists') return { text: item.conflictReason ?? '冲突', cls: 'bad' };
  if (item.state === 'unchanged') return { text: F.batchRenameUnchanged, cls: 'dim' };
  return { text: '', cls: '' };
}

export function BatchRenameDialog({ state }: { state: BatchRenameDialogState }) {
  const [mode, setMode] = useState<RenameMode>('dos');
  const [mask, setMask] = useState('IMG####.jpg');
  const [counterStart, setCounterStart] = useState(1);
  const [pattern, setPattern] = useState('');
  const [template, setTemplate] = useState('');

  const plan = useMemo(
    () =>
      planRenames({
        rows: state.rows,
        allRows: state.allRows,
        mode,
        dos: { mask, counterStart },
        regex: { pattern, template },
      }),
    [state.rows, state.allRows, mode, mask, counterStart, pattern, template]
  );

  const blocked = plan.conflicts > 0 || plan.renames.length === 0;
  const preview = plan.items.slice(0, PREVIEW_LIMIT);

  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={F.batchRenameTitle} style={{ width: 640, maxWidth: '92vw' }}>
        <h3 style={{ margin: '0 0 8px' }}>{F.batchRenameTitle}</h3>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 8 }}>
          <button className={`toggle${mode === 'dos' ? ' on' : ''}`} style={{ padding: '2px 12px' }} onClick={() => setMode('dos')}>
            {F.batchRenameModeDos}
          </button>
          <button className={`toggle${mode === 'regex' ? ' on' : ''}`} style={{ padding: '2px 12px' }} onClick={() => setMode('regex')}>
            {F.batchRenameModeRegex}
          </button>
          <span className="dim" style={{ fontSize: 12 }}>
            {mode === 'dos' ? F.batchRenameHintDos : F.batchRenameHintRegex}
          </span>
        </div>

        {mode === 'dos' ? (
          <div className="aic-dialog-row" style={{ alignItems: 'center' }}>
            <span className="dim" style={{ flex: 'none' }}>
              {F.batchRenameMaskLabel}
            </span>
            <input value={mask} onChange={(e) => setMask(e.target.value)} style={{ fontFamily: 'ui-monospace, Menlo, monospace' }} />
            <span className="dim" style={{ flex: 'none' }}>
              {F.batchRenameCounterLabel}
            </span>
            <input
              type="number"
              min={0}
              value={counterStart}
              onChange={(e) => setCounterStart(Math.max(0, Math.floor(Number(e.target.value) || 0)))}
              style={{ width: 72 }}
            />
          </div>
        ) : (
          <>
            <div className="aic-dialog-row">
              <span className="dim" style={{ flex: 'none' }}>
                {F.batchRenamePatternLabel}
              </span>
              <input
                value={pattern}
                placeholder="^(old_)(.*)$"
                onChange={(e) => setPattern(e.target.value)}
                style={{ fontFamily: 'ui-monospace, Menlo, monospace' }}
              />
            </div>
            <div className="aic-dialog-row">
              <span className="dim" style={{ flex: 'none' }}>
                {F.batchRenameTemplateLabel}
              </span>
              <input
                value={template}
                placeholder="new_$2"
                onChange={(e) => setTemplate(e.target.value)}
                style={{ fontFamily: 'ui-monospace, Menlo, monospace' }}
              />
            </div>
          </>
        )}

        <p style={{ margin: '8px 0 4px' }}>
          {F.batchRenamePreviewTitle}（{state.rows.length} 项）
        </p>
        <div
          style={{
            maxHeight: 260,
            overflow: 'auto',
            border: '1px solid var(--aic-border)',
            borderRadius: 4,
            fontSize: 12,
          }}
        >
          <table style={{ width: '100%', borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ textAlign: 'left', color: 'var(--aic-text-dim)' }}>
                <th style={thStyle}>{F.batchRenameColOld}</th>
                <th style={thStyle}>{F.batchRenameColNew}</th>
                <th style={thStyle}>{F.batchRenameColNote}</th>
              </tr>
            </thead>
            <tbody>
              {preview.map((item, i) => {
                const note = stateNote(item);
                return (
                  <tr key={`${item.row.id}-${i}`} style={{ borderTop: '1px solid var(--aic-border)' }}>
                    <td style={tdStyle}>{item.oldName}</td>
                    <td style={{ ...tdStyle, fontFamily: 'ui-monospace, Menlo, monospace' }}>{item.newName ?? '—'}</td>
                    <td
                      style={{
                        ...tdStyle,
                        color: note.cls === 'bad' ? 'var(--aic-st-diff, #d33)' : 'var(--aic-text-dim)',
                      }}
                    >
                      {note.text || (item.state === 'rename' ? '✓' : item.state === 'unchanged' ? F.batchRenameUnchanged : F.batchRenameKeepOriginal)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {plan.items.length > preview.length && <p className="dim" style={{ margin: '4px 8px' }}>{S.folder.planPreviewMore(plan.items.length - preview.length)}</p>}
        </div>

        <div className="aic-plan-stats" style={{ marginTop: 8 }}>
          <span>
            <b>{F.batchRenameExecCount(plan.renames.length)}</b>
          </span>
          {plan.conflicts > 0 && (
            <span style={{ color: 'var(--aic-st-diff, #d33)' }}>
              <b>{F.batchRenameConflictCount(plan.conflicts)}</b>
            </span>
          )}
          {plan.unchanged > 0 && (
            <span>
              <b>{`${F.batchRenameUnchanged} ${plan.unchanged} 项`}</b>
            </span>
          )}
        </div>

        <div className="aic-dialog-actions">
          <button onClick={state.onCancel}>{S.folder.confirmCancel}</button>
          <button
            className="primary"
            disabled={blocked}
            title={plan.renames.length === 0 ? F.batchRenameNoSelection : undefined}
            onClick={() => state.onOk(plan.renames.map((r) => ({ row: r.row, newName: r.newName! })))}
          >
            {S.folder.confirmExecute}
          </button>
        </div>
      </div>
    </div>
  );
}

const thStyle: React.CSSProperties = { padding: '4px 8px', position: 'sticky', top: 0, background: 'var(--aic-bg-panel)' };
const tdStyle: React.CSSProperties = { padding: '3px 8px', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 260 };
