import { useEffect, useRef } from 'react';
import { S } from '../../strings';

/**
 * 单窗格查找替换条（FR-TXT-022 / FIND-001 复用）。
 *
 * 匹配计算在视图层走 `lib/findCore`（与双栏比较同一份纯核心），本组件只负责
 * 输入与按钮；与 `views/text-compare/FindPanel` 的差别是没有"侧选择"（单文件
 * 只有一个文档），因此不硬塞双栏面板。
 */

export interface EditFindState {
  needle: string;
  replacement: string;
  regex: boolean;
  caseSensitive: boolean;
  wholeWord: boolean;
}

export interface EditFindBarProps {
  state: EditFindState;
  onChange(next: EditFindState): void;
  /** total=-1 = 未输入；error 非 null = 正则非法 */
  result: { total: number; index: number; error: string | null };
  onFind(backward: boolean): void;
  onReplaceCurrent(): void;
  onReplaceAll(): void;
  onClose(): void;
}

function Toggle({ label, active, onClick }: { label: string; active: boolean; onClick(): void }) {
  return (
    <button className={`aic-find-toggle${active ? ' on' : ''}`} onClick={onClick}>
      {label}
    </button>
  );
}

export function EditFindBar({
  state,
  onChange,
  result,
  onFind,
  onReplaceCurrent,
  onReplaceAll,
  onClose,
}: EditFindBarProps) {
  const needleRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    needleRef.current?.focus();
  }, []);

  const patch = (p: Partial<EditFindState>): void => onChange({ ...state, ...p });

  return (
    <div className="aic-find-panel aic-edit-find">
      <div className="aic-find-row">
        <input
          ref={needleRef}
          className="aic-find-input"
          placeholder={S.edit.findPlaceholder}
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
            <span className="aic-find-error">
              {S.edit.findRegexError}: {result.error}
            </span>
          ) : result.total > 0 ? (
            `${result.index + 1}/${result.total}`
          ) : result.total === 0 ? (
            S.edit.findNoMatch
          ) : null}
        </span>
        <button className="nav" onClick={() => onFind(true)} title={S.edit.findPrev}>
          ↑
        </button>
        <button className="nav" onClick={() => onFind(false)} title={S.edit.findNext}>
          ↓
        </button>
        <button className="nav" onClick={onClose} title={S.edit.findClose}>
          ✕
        </button>
      </div>
      <div className="aic-find-row">
        <input
          className="aic-find-input"
          placeholder={S.edit.replacePlaceholder}
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
          {S.edit.replaceCurrent}
        </button>
        <button className="nav" onClick={onReplaceAll} disabled={result.total <= 0}>
          {S.edit.replaceAll}
        </button>
      </div>
      <div className="aic-find-row aic-find-opts">
        <Toggle label=".*" active={state.regex} onClick={() => patch({ regex: !state.regex })} />
        <Toggle
          label={S.edit.findCaseSensitive}
          active={state.caseSensitive}
          onClick={() => patch({ caseSensitive: !state.caseSensitive })}
        />
        <Toggle
          label={S.edit.findWholeWord}
          active={state.wholeWord}
          onClick={() => patch({ wholeWord: !state.wholeWord })}
        />
      </div>
    </div>
  );
}
