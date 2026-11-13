/**
 * 基文件夹导航栏（FR-DIR-003，P6b）：可编辑面包屑。
 * - 每侧路径渲染为层级面包屑：点击某层级 = 以该层作为该侧基文件夹打开
 *   新会话（store 动作 openFolderTab 已备，视图层接线；新标签 = 新会话）；
 * - 每侧「上一级」（↑）与「编辑路径」（✎，回车提交/Escape 取消）；
 * - 尾部「交换侧」（⇄）：左右基互换开新会话（FR-DIR-003 交换侧）。
 * 仅操作 folder-compare 视图内部状态，不触碰 store 文件。
 */

import { useState } from 'react';
import { F } from './localStrings';

export interface BasePathBarProps {
  left: string;
  right: string;
  /** 打开新会话（newLeft/newRight 已是完整基路径；空串校验由调用方处理） */
  onOpen(newLeft: string, newRight: string): void;
}

function splitPath(p: string): { root: boolean; segs: string[] } {
  const root = p.startsWith('/') || p.startsWith('\\');
  const segs = p.split(/[\\/]+/).filter((s) => s.length > 0);
  return { root, segs };
}

function joinPath(root: boolean, segs: readonly string[]): string {
  if (segs.length === 0) return root ? '/' : '';
  return root ? `/${segs.join('/')}` : segs.join('/');
}

/** 第 count 层前缀路径（count >= 1） */
export function prefixPath(p: string, count: number): string {
  const { root, segs } = splitPath(p);
  return joinPath(root, segs.slice(0, Math.max(1, count)));
}

export function parentPath(p: string): string {
  const { root, segs } = splitPath(p);
  if (segs.length === 0) return p;
  return joinPath(root, segs.slice(0, -1));
}

const chipStyle: React.CSSProperties = {
  border: 'none',
  background: 'transparent',
  color: 'inherit',
  cursor: 'pointer',
  padding: '0 2px',
  font: 'inherit',
};
const actionStyle: React.CSSProperties = {
  border: '1px solid var(--aic-border)',
  background: 'transparent',
  borderRadius: 4,
  cursor: 'pointer',
  font: 'inherit',
  fontSize: 11,
  padding: '0 6px',
  lineHeight: '18px',
};

export function BasePathBar({ left, right, onOpen }: BasePathBarProps) {
  const [editing, setEditing] = useState<'left' | 'right' | null>(null);
  const [draft, setDraft] = useState('');

  const openWith = (side: 'left' | 'right', value: string): void => {
    onOpen(side === 'left' ? value : left, side === 'right' ? value : right);
  };

  const startEdit = (side: 'left' | 'right'): void => {
    setEditing(side);
    setDraft(side === 'left' ? left : right);
  };

  const commitEdit = (side: 'left' | 'right'): void => {
    const v = draft.trim();
    setEditing(null);
    if (v) openWith(side, v);
  };

  const renderSide = (side: 'left' | 'right'): React.ReactNode => {
    const value = side === 'left' ? left : right;
    const { root, segs } = splitPath(value);
    const sideLabel = side === 'left' ? F.navLeft : F.navRight;
    if (editing === side) {
      return (
        <span key={side} style={{ display: 'inline-flex', alignItems: 'center', gap: 4, minWidth: 220 }}>
          <span
            style={{
              fontSize: 10,
              border: '1px solid var(--aic-border)',
              borderRadius: 3,
              padding: '0 4px',
              color: 'var(--aic-text-dim)',
              flex: 'none',
            }}
          >
            {sideLabel}
          </span>
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitEdit(side);
              if (e.key === 'Escape') setEditing(null);
            }}
            onBlur={() => setEditing(null)}
            style={{ flex: 1, minWidth: 180, fontFamily: 'ui-monospace, Menlo, monospace', fontSize: 12 }}
          />
        </span>
      );
    }
    return (
      <span key={side} style={{ display: 'inline-flex', alignItems: 'center', gap: 2, minWidth: 0, overflow: 'hidden' }}>
        <span
          style={{
            fontSize: 10,
            border: '1px solid var(--aic-border)',
            borderRadius: 3,
            padding: '0 4px',
            color: 'var(--aic-text-dim)',
            flex: 'none',
          }}
        >
          {sideLabel}
        </span>
        {root && (
          <button style={chipStyle} title={F.navOpenHere(sideLabel)} onClick={() => openWith(side, prefixPath(value, 0) === '/' ? '/' : joinPath(root, []))}>
            /
          </button>
        )}
        {segs.map((seg, i) => (
          <span key={`${seg}-${i}`} style={{ display: 'inline-flex', alignItems: 'center', minWidth: 0 }}>
            {i > 0 && <span style={{ color: 'var(--aic-text-dim)' }}>/</span>}
            <button
              style={{ ...chipStyle, whiteSpace: 'nowrap' }}
              title={F.navOpenHere(sideLabel)}
              onClick={() => openWith(side, prefixPath(value, i + 1))}
            >
              {seg}
            </button>
          </span>
        ))}
        <button style={{ ...actionStyle, marginLeft: 4, flex: 'none' }} title={F.navUp(sideLabel)} onClick={() => openWith(side, parentPath(value))}>
          ↑
        </button>
        <button style={{ ...actionStyle, flex: 'none' }} title={F.navEdit(sideLabel)} onClick={() => startEdit(side)}>
          ✎
        </button>
      </span>
    );
  };

  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 10, minWidth: 0, flex: 1, overflow: 'hidden' }}>
      {renderSide('left')}
      {renderSide('right')}
      <button
        style={{ ...actionStyle, flex: 'none' }}
        title={F.navSwap}
        onClick={() => onOpen(right, left)}
      >
        ⇄
      </button>
    </span>
  );
}
