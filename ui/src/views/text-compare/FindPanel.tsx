import { useEffect, useRef } from 'react';
import { S } from '../../strings';

/**
 * 查找替换面板（FIND-001 / FR-TXT-022）：
 * 双窗格查找（侧选择）、正则/大小写/整词开关、匹配计数、
 * 下一个/上一个（循环）、替换（编辑态）、全部替换、全选匹配行。
 * 匹配计算与装饰在 TextCompareView 编排层（lib/findCore 纯核心）。
 */

export interface FindPanelState {
  needle: string;
  replacement: string;
  regex: boolean;
  caseSensitive: boolean;
  wholeWord: boolean;
  side: 'left' | 'right' | 'both';
}

export interface FindPanelProps {
  state: FindPanelState;
  onChange(next: FindPanelState): void;
  /** 匹配计数（total=-1 表示 needle 为空；error 非 null 为正则错误） */
  result: { total: number; index: number; error: string | null };
  onFind(backward: boolean): void;
  onReplaceCurrent(): void;
  onReplaceAll(): void;
  onSelectAllMatchedLines(): void;
  onClose(): void;
}

function Toggle({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick(): void;
}) {
  return (
    <button className={`aic-find-toggle${active ? ' on' : ''}`} onClick={onClick}>
      {label}
    </button>
  );
}

export function FindPanel({
  state,
  onChange,
  result,
  onFind,
  onReplaceCurrent,
  onReplaceAll,
  onSelectAllMatchedLines,
  onClose,
}: FindPanelProps) {
  const needleRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    needleRef.current?.focus();
  }, []);

  const patch = (p: Partial<FindPanelState>): void => onChange({ ...state, ...p });

  return (
    <div className="aic-find-panel">
      <div className="aic-find-row">
        <input
          ref={needleRef}
          className="aic-find-input"
          placeholder={S.text.findPlaceholder}
          value={state.needle}
          onChange={(e) => patch({ needle: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              onFind(e.shiftKey);
            } else if (e.key === 'Escape') {
              onClose();
            }
          }}
        />
        <span className="aic-find-count">
          {result.error ? (
            <span className="aic-find-error">{S.text.findRegexError}: {result.error}</span>
          ) : result.total > 0 ? (
            `${result.index + 1}/${result.total}`
          ) : result.total === 0 ? (
            S.text.findNoMatch
          ) : null}
        </span>
        <button className="nav" onClick={() => onFind(true)} title={S.text.findPrev}>
          ↑
        </button>
        <button className="nav" onClick={() => onFind(false)} title={S.text.findNext}>
          ↓
        </button>
        <button className="nav" onClick={onClose} title={S.text.findClose}>
          ✕
        </button>
      </div>
      <div className="aic-find-row">
        <input
          className="aic-find-input"
          placeholder={S.text.replacePlaceholder}
          value={state.replacement}
          onChange={(e) => patch({ replacement: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              onReplaceCurrent();
            } else if (e.key === 'Escape') {
              onClose();
            }
          }}
        />
        <button className="nav" onClick={onReplaceCurrent} disabled={result.total <= 0}>
          {S.text.replaceCurrent}
        </button>
        <button className="nav" onClick={onReplaceAll} disabled={result.total <= 0}>
          {S.text.replaceAll}
        </button>
        <button className="nav" onClick={onSelectAllMatchedLines} disabled={result.total <= 0}>
          {S.text.selectAllMatches}
        </button>
      </div>
      <div className="aic-find-row aic-find-opts">
        <Toggle label=".*" active={state.regex} onClick={() => patch({ regex: !state.regex })} />
        <Toggle
          label={S.text.findCaseSensitive}
          active={state.caseSensitive}
          onClick={() => patch({ caseSensitive: !state.caseSensitive })}
        />
        <Toggle
          label={S.text.findWholeWord}
          active={state.wholeWord}
          onClick={() => patch({ wholeWord: !state.wholeWord })}
        />
        <span className="aic-find-side dim">{S.text.findSide}</span>
        <select
          value={state.side}
          onChange={(e) => patch({ side: e.target.value as FindPanelState['side'] })}
        >
          <option value="left">{S.text.paneLeft}</option>
          <option value="right">{S.text.paneRight}</option>
          <option value="both">{S.text.findBothSides}</option>
        </select>
        <span className="dim aic-find-hint">{S.text.findHint}</span>
      </div>
    </div>
  );
}
