/**
 * 快速比较对话框（FR-DIR-013，P6b）：
 * - 选中一行：该行左右两侧互相比较（与对面项目成对比较）；
 * - 选中两行：两项互相比较（无论是否相对）；
 * - 展示两侧 size/mtime 与差异结论；说明文案标注「结果是否为快速判定」
 *   （status=quickDiff/same → 仅快速项判定未读内容；contentDiff → 基于内容比较）。
 * 数据全部来自 DirRow 现有字段（size/mtimeMs/status），无新 RPC。
 */

import type { DirRow, SideInfo } from '../../protocol/types';
import { formatSize, formatTime } from '../../lib/env';
import { S } from '../../strings';
import { F } from './localStrings';

export interface QuickCompareDialogState {
  /** 1 或 2 行（更多行由视图层限制） */
  rows: readonly DirRow[];
  onClose(): void;
}

interface PairSide {
  label: string;
  name: string;
  size: number | null;
  mtimeMs: number | null;
}

/** 单行模式：取该行左右两侧 */
function sameRowSides(row: DirRow): { a: PairSide; b: PairSide } {
  return {
    a: toSide(row.left, `${S.folder.leftTag} · ${row.name}`),
    b: toSide(row.right, `${S.folder.rightTag} · ${row.name}`),
  };
}

/** 两行模式：各取存在侧（左优先；孤立项取其存在侧） */
function crossRowSides(rows: readonly DirRow[]): { a: PairSide; b: PairSide } {
  const [r1, r2] = rows;
  const a = toSide(r1.left ?? r1.right, `A · ${r1.left ? S.folder.leftTag : S.folder.rightTag} · ${r1.name}`, r1.name);
  const b = toSide(r2.left ?? r2.right, `B · ${r2.left ? S.folder.leftTag : S.folder.rightTag} · ${r2.name}`, r2.name);
  return { a, b };
}

function toSide(info: SideInfo | null, label: string, name = ''): PairSide {
  return { label, name, size: info?.size ?? null, mtimeMs: info?.mtimeMs ?? null };
}

export function QuickCompareDialog({ state }: { state: QuickCompareDialogState }) {
  const { a, b } = state.rows.length === 2 ? crossRowSides(state.rows) : sameRowSides(state.rows[0]);
  const comparable = a.size !== null && b.size !== null && a.mtimeMs !== null && b.mtimeMs !== null;
  const sizeDiff = comparable && a.size !== b.size;
  const timeDiff = comparable && a.mtimeMs !== b.mtimeMs;

  // 结果是否为快速判定（FR-DIR-010/011/013 说明文案）：
  // quickDiff = 引擎仅按快速项（大小/时间戳）判异，未读取内容；contentDiff = 已做内容比较
  const statuses = state.rows.map((r) => r.status);
  const isContent = statuses.includes('contentDiff');
  const verdict = isContent ? F.quickCompareContentVerdict : F.quickCompareQuickVerdict;

  const cellStyle: React.CSSProperties = { padding: '4px 10px', borderTop: '1px solid var(--aic-border)', whiteSpace: 'nowrap' };
  const labelStyle: React.CSSProperties = { ...cellStyle, color: 'var(--aic-text-dim)' };
  const diffColor = 'var(--aic-st-diff, #d33)';

  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={F.quickCompareTitle} style={{ width: 560, maxWidth: '92vw' }}>
        <h3 style={{ margin: '0 0 4px' }}>{F.quickCompareTitle}</h3>
        <p className="dim" style={{ margin: '0 0 8px', fontSize: 12 }}>
          {F.quickCompareDesc}
        </p>
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 12 }}>
          <thead>
            <tr style={{ textAlign: 'left', color: 'var(--aic-text-dim)' }}>
              <th style={{ padding: '2px 10px' }}></th>
              <th style={{ padding: '2px 10px' }}>{a.label}</th>
              <th style={{ padding: '2px 10px' }}>{b.label}</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td style={labelStyle}>{S.folder.nameCol}</td>
              <td style={cellStyle}>{a.name || '—'}</td>
              <td style={cellStyle}>{b.name || '—'}</td>
            </tr>
            <tr>
              <td style={labelStyle}>{S.folder.sizeCol}</td>
              <td style={cellStyle}>{a.size === null ? F.quickCompareMissing : formatSize(a.size)}</td>
              <td style={cellStyle}>{b.size === null ? F.quickCompareMissing : formatSize(b.size)}</td>
            </tr>
            <tr>
              <td style={labelStyle}>{S.folder.mtimeCol}</td>
              <td style={cellStyle}>{a.mtimeMs === null ? F.quickCompareMissing : formatTime(a.mtimeMs)}</td>
              <td style={cellStyle}>{b.mtimeMs === null ? F.quickCompareMissing : formatTime(b.mtimeMs)}</td>
            </tr>
            <tr>
              {/* 本地文案：比较判定（待并入 strings.ts） */}
              <td style={labelStyle}>判定</td>
              <td style={cellStyle} colSpan={2}>
                {comparable ? (
                  <>
                    <span style={sizeDiff ? { color: diffColor } : undefined}>{sizeDiff ? F.quickCompareDiffSize : F.quickCompareSameSize}；</span>
                    <span style={timeDiff ? { color: diffColor } : undefined}>{timeDiff ? F.quickCompareDiffTime : F.quickCompareSameTime}</span>
                  </>
                ) : (
                  <span className="dim">—（{F.quickCompareMissing}）</span>
                )}
              </td>
            </tr>
          </tbody>
        </table>
        <div className="aic-plan-warning" style={{ marginTop: 10 }}>
          {verdict}
        </div>
        <p className="dim" style={{ margin: '8px 0 0', fontSize: 12 }}>
          {state.rows.length === 2 ? F.quickComparePairVerdict : F.quickCompareSingleVerdict}
        </p>
        <div className="aic-dialog-actions">
          <button onClick={state.onClose}>关闭</button>
        </div>
      </div>
    </div>
  );
}
