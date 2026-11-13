import { useEffect, useRef } from 'react';
import { Compartment, EditorState, type Extension, type Text } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import type { AlignedRow } from './alignment';
import type { InlineMark } from '../../lib/inlineDiffCore';
import {
  buildPaneDecorations,
  buildStaticExtensions,
  type FindMark,
  type Importance,
  type PaneViewData,
} from './cmDecorations';
import type { FoldRange } from './folding';

/**
 * 对齐文本窗格（M32，P5 可编辑态）。
 *
 * 生命周期：组件挂载创建唯一 EditorView（常驻）；数据更新经 setContent
 * 以"全量文档替换 + 装饰 compartment 重配"驱动（编辑重 diff / 折叠 /
 * 查找 / 导航高亮统一走该入口），不做视图销毁重建（保留滚动与焦点）。
 * 用户编辑由 updateListener 上报父级编排层（editPatch → 重 diff → 回灌）。
 */

export interface PaneContentInput {
  /** 对齐文档文本（AlignedDoc.lines.join('\n')） */
  docText: string;
  rows: AlignedRow[];
  sectionImportance: Map<number, Importance>;
  marks: InlineMark[];
  folds: FoldRange[];
  findMarks: FindMark[];
  currentSectionId: number | null;
  /** 语法高亮语言扩展（null = 纯文本） */
  langExtension: Extension | null;
  editable: boolean;
}

export interface AlignedPaneHooks {
  onUndo(): void;
  onRedo(): void;
  /** 用户编辑上报（side, 编辑前对齐行元数据, 编辑前文档行, 编辑后文档行） */
  onUserEdit(side: 'left' | 'right', rows: AlignedRow[], docBefore: string[], docAfter: string[]): void;
  /** 滚动上报（滚动同步 + 概览条视口 + 差异列表当前位置） */
  onScroll(side: 'left' | 'right'): void;
  /** 折叠占位条点击展开 */
  onUnfold(fromRow: number, toRow: number): void;
  /** 光标/选择变化（查找起点跟随） */
  onSelection(side: 'left' | 'right'): void;
}

export interface AlignedPaneHandle {
  setContent(input: PaneContentInput): void;
  getView(): EditorView | null;
  /** 0 基对齐行号 → 居中滚动 */
  revealLine(line: number): void;
  /** 当前视口顶部可见的对齐行号（0 基） */
  topVisibleLine(): number;
  /** 当前视口底部可见的对齐行号（0 基，含） */
  bottomVisibleLine(): number;
  /** 选中一行内区间（查找定位；0 基行号 + 字符区间） */
  selectRange(line: number, from: number, to: number): void;
  /** 当前光标行（0 基）与列 */
  cursorPos(): { line: number; col: number };
  focusPane(): void;
}

export interface AlignedPaneProps {
  side: 'left' | 'right';
  head: string;
  initial: PaneContentInput;
  hooks: AlignedPaneHooks;
  registerHandle(handle: AlignedPaneHandle | null): void;
}

function docLines(doc: Text): string[] {
  const out: string[] = [];
  for (let i = 1; i <= doc.lines; i++) out.push(doc.line(i).text);
  return out;
}

export function AlignedPane({ side, head, initial, hooks, registerHandle }: AlignedPaneProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const dataRef = useRef<PaneContentInput>(initial);
  const programmaticRef = useRef(false);
  const hooksRef = useRef(hooks);
  hooksRef.current = hooks;
  const langCompRef = useRef(new Compartment());
  const decorCompRef = useRef(new Compartment());
  const editableCompRef = useRef(new Compartment());

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const langComp = langCompRef.current;
    const decorComp = decorCompRef.current;
    const editableComp = editableCompRef.current;

    const { statics, dataRef: gutterDataRef } = buildStaticExtensions(toViewData(initial), {
      onUndo: () => hooksRef.current.onUndo(),
      onRedo: () => hooksRef.current.onRedo(),
    });

    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initial.docText,
        extensions: [
          ...statics,
          decorComp.of(EditorView.decorations.of(buildPaneDecorations(toViewData(initial), initial.docText))),
          langComp.of(initial.langExtension ?? []),
          editableComp.of([
            EditorState.readOnly.of(!initial.editable),
            EditorView.editable.of(initial.editable),
          ]),
          EditorView.updateListener.of((update) => {
            if (update.docChanged && !programmaticRef.current) {
              const before = docLines(update.startState.doc);
              const after = docLines(update.state.doc);
              const rows = dataRef.current.rows;
              // 编辑事务同步上报（编排层在本 tick 内完成重 diff 回灌）
              hooksRef.current.onUserEdit(side, rows, before, after);
            }
            if (update.geometryChanged || update.docChanged) {
              hooksRef.current.onScroll(side);
            }
            if (update.selectionSet && !programmaticRef.current) {
              hooksRef.current.onSelection(side);
            }
          }),
        ],
      }),
    });
    viewRef.current = view;
    view.scrollDOM.addEventListener('scroll', () => hooksRef.current.onScroll(side));

    // 折叠占位条点击 → 展开（事件委托；widget ignoreEvent=false）
    const hostClick = (ev: MouseEvent): void => {
      const target = ev.target as HTMLElement | null;
      const fold = target?.closest?.('.cm-aic-fold-widget') as HTMLElement | null;
      if (fold) {
        const from = Number(fold.dataset.fromRow);
        const to = Number(fold.dataset.toRow);
        if (Number.isFinite(from) && Number.isFinite(to)) {
          ev.preventDefault();
          hooksRef.current.onUnfold(from, to);
        }
      }
    };
    host.addEventListener('click', hostClick);

    const handle: AlignedPaneHandle = {
      setContent(input) {
        const v = viewRef.current;
        if (!v) return;
        const prevData = dataRef.current;
        dataRef.current = input;
        gutterDataRef.current = toViewData(input); // gutter lineMarker 闭包的数据源同步
        const scrollTop = v.scrollDOM.scrollTop;
        const sel = v.state.selection.main;
        const prevLine = v.state.doc.lineAt(sel.head);
        const curCol = sel.head - prevLine.from;
        const curLine = prevLine.number - 1;

        programmaticRef.current = true;
        try {
          const effects = [
            decorComp.reconfigure(
              EditorView.decorations.of(buildPaneDecorations(toViewData(input), input.docText))
            ),
          ];
          if (prevData.langExtension !== input.langExtension) {
            effects.push(langComp.reconfigure(input.langExtension ?? []));
          }
          if (prevData.editable !== input.editable) {
            effects.push(
              editableComp.reconfigure([
                EditorState.readOnly.of(!input.editable),
                EditorView.editable.of(input.editable),
              ])
            );
          }
          v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: input.docText }, effects });
          // 恢复滚动与光标（编辑重 diff 后行号一般保持；越界时 clamp）
          v.scrollDOM.scrollTop = scrollTop;
          const lineNo = Math.min(curLine + 1, v.state.doc.lines);
          const line = v.state.doc.line(lineNo);
          const pos = Math.min(line.from + curCol, line.to);
          v.dispatch({ selection: { anchor: pos } });
        } finally {
          programmaticRef.current = false;
        }
      },
      getView: () => viewRef.current,
      revealLine(line) {
        const v = viewRef.current;
        if (!v || line < 0 || line >= v.state.doc.lines) return;
        const pos = v.state.doc.line(line + 1).from;
        // lineBlockAt 返回文档坐标（与 scrollTop 同一空间，CM6 文档顶 = 0）
        const top = v.lineBlockAt(pos).top;
        v.scrollDOM.scrollTop = Math.max(0, top - v.scrollDOM.clientHeight / 3);
      },
      topVisibleLine() {
        const v = viewRef.current;
        if (!v) return 0;
        const info = v.lineBlockAtHeight(v.scrollDOM.scrollTop);
        return v.state.doc.lineAt(info.from).number - 1;
      },
      bottomVisibleLine() {
        const v = viewRef.current;
        if (!v) return 0;
        const y = v.scrollDOM.scrollTop + v.scrollDOM.clientHeight - 2;
        const info = v.lineBlockAtHeight(y);
        return v.state.doc.lineAt(info.from).number - 1;
      },
      selectRange(line, from, to) {
        const v = viewRef.current;
        if (!v || line < 0 || line >= v.state.doc.lines) return;
        const l = v.state.doc.line(line + 1);
        const a = Math.min(l.from + from, l.to);
        const b = Math.min(l.from + to, l.to);
        v.dispatch({ selection: { anchor: a, head: b }, scrollIntoView: true });
        v.focus();
      },
      cursorPos() {
        const v = viewRef.current;
        if (!v) return { line: 0, col: 0 };
        const head = v.state.selection.main.head;
        const l = v.state.doc.lineAt(head);
        return { line: l.number - 1, col: head - l.from };
      },
      focusPane() {
        viewRef.current?.focus();
      },
    };
    registerHandle(handle);

    return () => {
      host.removeEventListener('click', hostClick);
      registerHandle(null);
      view.destroy();
      viewRef.current = null;
    };
    // 初始数据一次性注入（后续全部走 setContent）；hooks 经 ref 保持新鲜
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="aic-pane">
      <div className="aic-pane-head" title={head}>
        {head}
      </div>
      <div className="aic-pane-body" ref={hostRef} />
    </div>
  );
}

function toViewData(input: PaneContentInput): PaneViewData {
  return {
    rows: input.rows,
    sectionImportance: input.sectionImportance,
    marks: input.marks,
    folds: input.folds,
    findMarks: input.findMarks,
    currentSectionId: input.currentSectionId,
  };
}
