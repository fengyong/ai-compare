import { S } from '../../strings';
import type { Section } from '../../protocol/types';

/**
 * 差异列表面板（FR-TXT-030 导航增强）：
 * 全部差异 section 的可点击清单；当前位置（滚动/导航驱动）同步高亮；
 * 点击跳转到对应差异。清单随重 diff 自动刷新。
 */

export interface DiffListPanelProps {
  sections: Section[];
  /** 当前差异 section id（同步高亮） */
  currentId: number | null;
  onGoto(index: number): void;
}

const KIND_GLYPH: Record<Section['kind'], string> = {
  equal: '=',
  insert: '+',
  delete: '−',
  change: '≠',
};

function rangeText(sec: Section): string {
  const l = sec.left ? `L${sec.left.start + 1}${sec.left.end - sec.left.start > 1 ? `-${sec.left.end}` : ''}` : null;
  const r = sec.right ? `R${sec.right.start + 1}${sec.right.end - sec.right.start > 1 ? `-${sec.right.end}` : ''}` : null;
  if (l && r) return `${l} ⇄ ${r}`;
  return l ?? r ?? '';
}

export function DiffListPanel({ sections, currentId, onGoto }: DiffListPanelProps) {
  return (
    <div className="aic-diff-list">
      <div className="aic-diff-list-head">
        {S.text.diffListTitle}
        <span className="dim">（{sections.length}）</span>
      </div>
      <div className="aic-diff-list-body">
        {sections.length === 0 && <div className="aic-tree-empty">{S.text.diffListEmpty}</div>}
        {sections.map((sec, i) => (
          <button
            key={sec.id}
            className={`aic-diff-list-row${sec.id === currentId ? ' current' : ''}`}
            onClick={() => onGoto(i)}
            title={rangeText(sec)}
          >
            <span className={`glyph ${sec.important === 'unimportant' ? 'insig' : sec.kind}`}>
              {KIND_GLYPH[sec.kind] ?? '?'}
            </span>
            <span className="ranges">{rangeText(sec)}</span>
            <span className="idx">{i + 1}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
