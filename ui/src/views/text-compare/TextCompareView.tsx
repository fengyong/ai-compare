import { useEffect, useMemo, useRef, useState } from 'react';
import type { EditorView } from '@codemirror/view';
import type { Tab } from '../../shell/store';
import { openText } from '../../ipc/engineProcess';
import { S } from '../../strings';
import { buildAlignedDocs, buildAllInlineMarks } from './alignment';
import { loadTextSession, type LoadedTextSession } from './textSession';
import { AlignedPane } from './AlignedPane';

/**
 * 文本比较视图（docs/03 §3.4.3 M32 的 P0 子集 / U2+U3，风险 R10 验证项）：
 * - CM6 双窗格，section 展开为逐行 1:1 对齐文档（对侧缺失行插占位填充行）
 * - 行级差异着色（浅红/浅蓝底纹 + gutter 彩点）、行内 span 级高亮
 * - 双栏同步滚动（行高等价时 scrollTop 镜像；导航用 section 锚点映射）
 * - 差异导航（上一处/下一处）；概览条/查找/编辑属 P5
 */
export function TextCompareView({ tab }: { tab: Tab }) {
  const text = tab.text!;
  const [session, setSession] = useState<LoadedTextSession | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [navIndex, setNavIndex] = useState(-1);

  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const opened = await openText({
          left: text.left,
          right: text.right,
          parent:
            text.parentFolderSessionId !== undefined && text.parentRowId !== undefined
              ? { sessionId: text.parentFolderSessionId, rowId: text.parentRowId }
              : undefined,
        });
        const loaded = await loadTextSession(opened);
        if (!disposed) setSession(loaded);
      } catch (err) {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      disposed = true;
    };
  }, [text.sessionId, text.left, text.right, text.parentFolderSessionId, text.parentRowId]);

  const aligned = useMemo(() => {
    if (!session) return null;
    const docs = buildAlignedDocs(session.sections, session.leftLines, session.rightLines);
    const marks = buildAllInlineMarks(session.sections, session.leftLines, session.rightLines);
    const importance = new Map<number, 'important' | 'unimportant' | 'ignored'>();
    for (const s of session.sections) importance.set(s.id, s.importance === 'unimportant' ? 'unimportant' : 'important');
    return { ...docs, marks, importance };
  }, [session]);

  // ---- 同步滚动与导航 ----
  const viewL = useRef<EditorView | null>(null);
  const viewR = useRef<EditorView | null>(null);
  const syncingRef = useRef(false);

  const mirrorScroll = (fromEl: HTMLElement): void => {
    if (syncingRef.current) return;
    const toEl = fromEl === viewL.current?.scrollDOM ? viewR.current?.scrollDOM : viewL.current?.scrollDOM;
    if (!toEl) return;
    syncingRef.current = true;
    toEl.scrollTop = fromEl.scrollTop;
    toEl.scrollLeft = fromEl.scrollLeft;
    // 释放guard：等程序化滚动的事件循环走完
    requestAnimationFrame(() => {
      syncingRef.current = false;
    });
  };

  /** 跳到对齐文档的第 rowIndex 行（两侧同时居中） */
  const scrollToRow = (rowIndex: number): void => {
    for (const v of [viewL.current, viewR.current]) {
      if (!v || rowIndex < 0 || rowIndex >= v.state.doc.lines) continue;
      const pos = v.state.doc.line(rowIndex + 1).from;
      const top = v.lineBlockAt(pos).top;
      v.scrollDOM.scrollTop = Math.max(0, top - v.scrollDOM.clientHeight / 3);
    }
  };

  const diffSections = useMemo(
    () => (session ? session.sections.filter((s) => s.kind !== 'equal') : []),
    [session]
  );

  const gotoSection = (idx: number): void => {
    if (!aligned || diffSections.length === 0) return;
    const wrapped = ((idx % diffSections.length) + diffSections.length) % diffSections.length;
    const sec = diffSections[wrapped];
    // 对齐文档中该 section 首行 = 左侧起始行号（左侧缺行时用右侧）
    const firstRow = sec.left?.start ?? sec.right?.start ?? 0;
    setNavIndex(wrapped);
    scrollToRow(firstRow);
  };

  if (error) {
    return (
      <div className="aic-text">
        <div className="aic-error-box">
          <b>{S.text.loadFail}</b>
          <pre>{error}</pre>
        </div>
      </div>
    );
  }

  if (!session || !aligned) {
    return (
      <div className="aic-text">
        <div className="aic-loading">{S.text.loading}</div>
      </div>
    );
  }

  const summary = session.summary;

  return (
    <div className="aic-text">
      <div className="aic-text-toolbar">
        <span className="fname" title={text.left.path ?? text.left.name}>
          {text.left.name}
        </span>
        <span className="meta">⇄</span>
        <span className="fname" title={text.right.path ?? text.right.name}>
          {text.right.name}
        </span>
        <span className="meta">
          {session.left.encoding} · {session.left.lineCount.toLocaleString('zh-CN')} {S.text.linesUnit}
        </span>
        <span className="spacer" style={{ flex: 1 }} />
        <span className="meta">
          {S.text.sectionsLabel} <b>{summary.diffSections}</b> / {S.text.equalSectionsLabel}{' '}
          <b>{summary.sections - summary.diffSections}</b>
        </span>
        <button className="nav" onClick={() => gotoSection(0)} disabled={diffSections.length === 0}>
          {S.text.firstDiff}
        </button>
        <button className="nav" onClick={() => gotoSection((navIndex === -1 ? 0 : navIndex) - 1)} disabled={diffSections.length === 0}>
          {S.text.prevDiff}
        </button>
        <button
          className="nav"
          onClick={() => gotoSection(navIndex === -1 ? 1 : navIndex + 1)}
          disabled={diffSections.length === 0}
        >
          {S.text.nextDiff}
          {navIndex >= 0 ? `（${navIndex + 1}/${diffSections.length}）` : ''}
        </button>
      </div>

      <div className="aic-text-panes">
        <AlignedPane
          head={`${S.text.paneLeft} · ${text.left.name}`}
          doc={aligned.left}
          marks={aligned.marks.left}
          sectionImportance={aligned.importance}
          onScroll={mirrorScroll}
          registerView={(v) => (viewL.current = v)}
        />
        <AlignedPane
          head={`${S.text.paneRight} · ${text.right.name}`}
          doc={aligned.right}
          marks={aligned.marks.right}
          sectionImportance={aligned.importance}
          onScroll={mirrorScroll}
          registerView={(v) => (viewR.current = v)}
        />
      </div>
    </div>
  );
}
