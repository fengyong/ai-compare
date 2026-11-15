/**
 * 目录操作对话框族（P6）：操作确认（含不可逆警示）/ 同步计划预览 / 重命名。
 * 确认体系对齐 OP-001/002：明确列出将执行清单与影响统计，删除/移动强制警示。
 */

import { useEffect, useRef, useState } from 'react';
import type { PlannedOperation, PlannedOpKind } from '../../protocol/types';
import { S } from '../../strings';
import type { DirExecuteOp, OpSummary } from './dirOps';

/** 计划/批次清单行（op 分类着色：复制蓝、复制回绿、删除红） */
function PlanRows({ ops }: { ops: PlannedOperation[] }) {
  const opClass = (op: PlannedOpKind): string => {
    switch (op) {
      case 'copyLeftToRight':
        return 'ltr';
      case 'copyRightToLeft':
        return 'rtl';
      case 'deleteLeft':
      case 'deleteRight':
        return 'del';
      default:
        return '';
    }
  };
  return (
    <div className="aic-plan-list">
      {ops.map((p, i) => (
        <div className="row" key={`${p.op}-${p.path}-${i}`}>
          <span className={`op ${opClass(p.op)}`}>{p.op}</span>
          <span className="path">{p.path}</span>
          <span className="reason" title={p.reason}>
            {p.reason}
          </span>
        </div>
      ))}
    </div>
  );
}

function ExecRows({ ops }: { ops: DirExecuteOp[] }) {
  return (
    <div className="aic-plan-list">
      {ops.map((op, i) => (
        <div className="row" key={`${op.kind}-${op.path}-${op.side}-${i}`}>
          <span className="op">{op.kind}</span>
          <span className="path">{op.path}</span>
          <span className="reason">{op.side}</span>
        </div>
      ))}
    </div>
  );
}

function SummaryChips({ summary }: { summary: OpSummary }) {
  const items: string[] = [];
  if (summary.copies) items.push(S.folder.confirmSummaryCopies(summary.copies));
  if (summary.moves) items.push(S.folder.confirmSummaryMoves(summary.moves));
  if (summary.deletes) items.push(S.folder.confirmSummaryDeletes(summary.deletes));
  if (summary.touches) items.push(S.folder.confirmSummaryTouches(summary.touches));
  if (summary.mkdirs) items.push(`新建文件夹 ${summary.mkdirs} 项`);
  if (summary.renames) items.push(`重命名 ${summary.renames} 项`);
  return (
    <div className="aic-plan-stats">
      {items.map((t) => (
        <span key={t}>
          <b>{t}</b>
        </span>
      ))}
    </div>
  );
}

export interface ConfirmOpsDialogState {
  title: string;
  ops: DirExecuteOp[];
  summary: OpSummary;
  irreversible: boolean;
  onConfirm(): void;
  onCancel(): void;
}

/** 操作确认对话框：批次清单 + 影响统计 + 不可逆警示（OP-001） */
export function ConfirmOpsDialog({ state }: { state: ConfirmOpsDialogState }) {
  const preview = state.ops.slice(0, 12);
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={state.title}>
        <h3 style={{ margin: '0 0 8px' }}>{state.title}</h3>
        <p>{S.folder.confirmOpsListTitle}</p>
        <ExecRows ops={preview} />
        {state.ops.length > preview.length && (
          <p className="dim">{S.folder.planPreviewMore(state.ops.length - preview.length)}</p>
        )}
        <SummaryChips summary={state.summary} />
        {state.irreversible && (
          <div className="aic-plan-warning">{S.folder.confirmIrreversible}</div>
        )}
        <div className="aic-dialog-actions">
          <button onClick={state.onCancel}>{S.folder.confirmCancel}</button>
          <button className="primary" onClick={state.onConfirm}>
            {S.folder.confirmExecute}
          </button>
        </div>
      </div>
    </div>
  );
}

export interface PlanPreviewDialogState {
  presetLabel: string;
  plan: PlannedOperation[];
  onConfirm(): void;
  onCancel(): void;
}

/** 同步计划预览对话框：dir/operate 计划 + 影响统计（FR-DIR-067 入口） */
export function PlanPreviewDialog({ state }: { state: PlanPreviewDialogState }) {
  const s = state.plan.reduce(
    (acc, p) => {
      acc[p.op] = (acc[p.op] ?? 0) + 1;
      return acc;
    },
    {} as Record<PlannedOpKind, number>,
  );
  const stats: string[] = [];
  if (s.copyLeftToRight) stats.push(S.folder.planCopyLR(s.copyLeftToRight));
  if (s.copyRightToLeft) stats.push(S.folder.planCopyRL(s.copyRightToLeft));
  if (s.deleteLeft) stats.push(S.folder.planDelL(s.deleteLeft));
  if (s.deleteRight) stats.push(S.folder.planDelR(s.deleteRight));
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={S.folder.planPreviewTitle}>
        <h3 style={{ margin: '0 0 8px' }}>
          {S.folder.planPreviewTitle} · {state.presetLabel}
        </h3>
        <p>{S.folder.planPreviewDesc}</p>
        {state.plan.length === 0 ? (
          <p className="dim">{S.folder.planPreviewEmpty}</p>
        ) : (
          <PlanRows ops={state.plan.slice(0, 40)} />
        )}
        {state.plan.length > 40 && (
          <p className="dim">{S.folder.planPreviewMore(state.plan.length - 40)}</p>
        )}
        <div className="aic-plan-stats">
          {stats.map((t) => (
            <span key={t}>
              <b>{t}</b>
            </span>
          ))}
        </div>
        {(s.deleteLeft > 0 || s.deleteRight > 0) && (
          <div className="aic-plan-warning">{S.folder.confirmIrreversible}</div>
        )}
        <div className="aic-dialog-actions">
          <button onClick={state.onCancel}>{S.folder.confirmCancel}</button>
          <button className="primary" disabled={state.plan.length === 0} onClick={state.onConfirm}>
            {S.folder.confirmExecute}
          </button>
        </div>
      </div>
    </div>
  );
}

export interface RenameDialogState {
  currentName: string;
  onOk(newName: string): void;
  onCancel(): void;
}

/** 重命名对话框（FR-DIR-064 的 P6 单项形态；批量 DOS/正则属后续阶段） */
export function RenameDialog({ state }: { state: RenameDialogState }) {
  const [name, setName] = useState(state.currentName);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={S.folder.renameTitle}>
        <h3 style={{ margin: '0 0 8px' }}>{S.folder.renameTitle}</h3>
        <div className="aic-dialog-row">
          <span className="dim">{S.folder.renameLabel}</span>
          <input
            ref={inputRef}
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && name.trim()) state.onOk(name.trim());
              if (e.key === 'Escape') state.onCancel();
            }}
          />
        </div>
        <div className="aic-dialog-actions">
          <button onClick={state.onCancel}>{S.folder.confirmCancel}</button>
          <button
            className="primary"
            disabled={!name.trim() || name.trim() === state.currentName}
            onClick={() => state.onOk(name.trim())}
          >
            {S.folder.renameOk}
          </button>
        </div>
      </div>
    </div>
  );
}
