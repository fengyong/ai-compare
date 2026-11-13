import { EditorState, type Extension, type Range } from '@codemirror/state';
import {
  Decoration,
  EditorView,
  GutterMarker,
  WidgetType,
  gutter,
  keymap,
  lineNumbers,
  type DecorationSet,
} from '@codemirror/view';
import { standardKeymap } from '@codemirror/commands';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';
import type { AlignedRow } from './alignment';
import type { InlineMark } from '../../lib/inlineDiffCore';
import type { FoldRange } from './folding';

/**
 * CM6 "对齐层"装饰（docs/03 §3.4.3 M32，ADR-008：CM6 为编辑内核、差异布局自研）。
 *
 * P5 升级：可编辑态 + 动态装饰（数据经 pane 的 compartment 整体重配，
 * 事务由视图编排层驱动），语义（docs/01 附录 C.3）：
 * - 重要差异行：浅红底纹（cm-aic-line-diff），浅红优先于浅蓝
 * - 不重要差异行：浅蓝底纹（cm-aic-line-insig）
 * - 对侧缺失的填充行：更弱底纹（cm-aic-line-absent）+ gutter 空心点
 * - 行内 span 级差异：Mark 高亮（cm-aic-inline-diff）
 * - 当前差异 section：行侧标（cm-aic-line-current，导航/滚动同步高亮）
 * - 查找命中：cm-aic-find-match / 当前命中 cm-aic-find-current（FIND-001）
 * - 等值区折叠：replace 装饰 + 占位 widget（点击展开，FR-TXT-020~023）
 * - 语法高亮：Lezer 语言包（langPacks 按需加载）+ 暗色 HighlightStyle（对齐色板）
 */

export type Importance = 'important' | 'unimportant' | 'ignored';

/** 单个查找命中（对齐文档行坐标） */
export interface FindMark {
  line: number;
  from: number;
  to: number;
  current: boolean;
}

/** 窗格视图数据（装饰重建输入；由 AlignedPane.setContent 驱动） */
export interface PaneViewData {
  rows: AlignedRow[];
  sectionImportance: Map<number, Importance>;
  /** 行内差异 span（0 基对齐行号 → 字符区间） */
  marks: InlineMark[];
  /** 折叠隐藏区间（含两端对齐行） */
  folds: FoldRange[];
  /** 查找命中（本侧窗格的） */
  findMarks: FindMark[];
  /** 当前差异 section id（行侧标） */
  currentSectionId: number | null;
}

class StatusMarker extends GutterMarker {
  constructor(private readonly cls: string) {
    super();
  }
  toDOM(): HTMLElement {
    const span = document.createElement('span');
    span.className = `cm-aic-gutter-mark ${this.cls}`;
    return span;
  }
}

const MARKER_IMPORTANT = new StatusMarker('important');
const MARKER_UNIMPORTANT = new StatusMarker('unimportant');
const MARKER_ABSENT = new StatusMarker('absent');

function lineClassFor(status: AlignedRow['status'], importance: Importance): string | null {
  switch (status) {
    case 'equal':
      return null;
    case 'absent':
      return 'cm-aic-line-absent';
    case 'inserted':
    case 'deleted':
    case 'changed':
      return importance === 'unimportant' ? 'cm-aic-line-insig' : 'cm-aic-line-diff';
  }
}

function gutterMarkerFor(row: AlignedRow, importance: Importance): GutterMarker | null {
  switch (row.status) {
    case 'equal':
      return null;
    case 'absent':
      return MARKER_ABSENT;
    case 'inserted':
    case 'deleted':
    case 'changed':
      return importance === 'unimportant' ? MARKER_UNIMPORTANT : MARKER_IMPORTANT;
  }
}

/** 折叠占位条（点击展开该等值段） */
class FoldWidget extends WidgetType {
  constructor(
    readonly fromRow: number,
    readonly toRow: number,
    readonly hiddenLines: number
  ) {
    super();
  }
  toDOM(): HTMLElement {
    const span = document.createElement('span');
    span.className = 'cm-aic-fold-widget';
    span.textContent = `⋯ ${this.hiddenLines} 行相同，点击展开 ⋯`;
    span.dataset.fromRow = String(this.fromRow);
    span.dataset.toRow = String(this.toRow);
    return span;
  }
  ignoreEvent(): boolean {
    return false; // 让 DOM click 冒泡到窗格宿主（展开由视图层处理）
  }
}

/** 由 PaneViewData 构建全部装饰（行级 + mark + 折叠 + 查找） */
export function buildPaneDecorations(data: PaneViewData, docText: string): DecorationSet {
  const doc = EditorState.create({ doc: docText, extensions: [] }).doc;
  const ranges: Array<Range<Decoration>> = [];
  const importanceOf = (row: AlignedRow): Importance =>
    data.sectionImportance.get(row.sectionId) ?? 'important';

  // 行级装饰（含当前 section 侧标）
  for (let i = 0; i < data.rows.length && i < doc.lines; i++) {
    const row = data.rows[i];
    const imp = importanceOf(row);
    const cls = lineClassFor(row.status, imp);
    const line = doc.line(i + 1);
    const classes = [cls];
    if (data.currentSectionId !== null && row.sectionId === data.currentSectionId && row.status !== 'equal') {
      classes.push('cm-aic-line-current');
    }
    const joined = classes.filter(Boolean).join(' ');
    if (joined) {
      ranges.push(Decoration.line({ class: joined }).range(line.from));
    }
  }

  // 行内 mark
  for (const mk of data.marks) {
    if (mk.line + 1 > doc.lines) continue;
    const line = doc.line(mk.line + 1);
    const from = Math.min(line.from + mk.from, line.to);
    const to = Math.min(line.from + mk.to, line.to);
    if (to > from) {
      ranges.push(Decoration.mark({ class: 'cm-aic-inline-diff' }).range(from, to));
    }
  }

  // 查找命中
  for (const fm of data.findMarks) {
    if (fm.line + 1 > doc.lines) continue;
    const line = doc.line(fm.line + 1);
    const from = Math.min(line.from + fm.from, line.to);
    const to = Math.min(line.from + fm.to, line.to);
    if (to > from) {
      ranges.push(
        Decoration.mark({ class: fm.current ? 'cm-aic-find-current' : 'cm-aic-find-match' }).range(from, to)
      );
    }
  }

  // 折叠（replace 多行区间 + 占位 widget；差异/上下文行不在区间内）
  for (const f of data.folds) {
    if (f.fromRow > f.toRow) continue;
    const fromLineNo = Math.min(f.fromRow + 1, doc.lines);
    const toLineNo = Math.min(f.toRow + 1, doc.lines);
    const from = doc.line(fromLineNo).from;
    const to = doc.line(toLineNo).to;
    if (to > from) {
      ranges.push(
        Decoration.replace({
          widget: new FoldWidget(f.fromRow, f.toRow, f.toRow - f.fromRow + 1),
        }).range(from, to)
      );
    }
  }

  return Decoration.set(ranges, true);
}

export interface PaneHooks {
  /** 会话级撤销（UNDO-001，跨窗格同栈） */
  onUndo(): void;
  onRedo(): void;
}

/** 构建单侧窗格的全部 CM6 扩展（静态部分；装饰经 compartment 单独重配） */
export function buildStaticExtensions(
  data: PaneViewData,
  hooks: PaneHooks
): { statics: Extension[]; dataRef: { current: PaneViewData } } {
  // gutter 的 lineMarker 闭包读 dataRef（重配后 rows 更新，gutter 扩展本身保持稳定）
  const dataRef: { current: PaneViewData } = { current: data };
  const statusGutter = gutter({
    class: 'cm-aic-status-gutter',
    lineMarker: (view, line) => {
      const d = dataRef.current;
      const lineNumber = view.state.doc.lineAt(line.from).number;
      const row = d.rows[lineNumber - 1];
      if (!row) return null;
      return gutterMarkerFor(row, d.sectionImportance.get(row.sectionId) ?? 'important');
    },
  });

  const statics: Extension[] = [
    lineNumbers(),
    statusGutter,
    editorTheme,
    syntaxHighlighting(aicHighlightStyle, { fallback: true }),
    keymap.of([
      { key: 'Mod-z', run: () => (hooks.onUndo(), true) },
      { key: 'Mod-y', run: () => (hooks.onRedo(), true) },
      { key: 'Mod-Shift-z', run: () => (hooks.onRedo(), true) },
    ]),
    keymap.of(standardKeymap),
    EditorState.readOnly.of(false),
    EditorView.editable.of(true),
  ];

  return { statics, dataRef };
}

/** 编辑器暗色主题（颜色引用 palette 注入的 CSS 变量） */
export const editorTheme = EditorView.theme(
  {
    '&': {
      height: '100%',
      backgroundColor: 'var(--aic-editor-bg)',
      color: 'var(--aic-editor-text)',
    },
    '.cm-scroller': {
      fontFamily: "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
      lineHeight: '1.5',
      overflow: 'auto',
    },
    '.cm-content': {
      caretColor: 'var(--aic-accent)',
    },
    '&.cm-focused': {
      outline: 'none',
    },
    '.cm-cursor': {
      borderLeftColor: 'var(--aic-accent)',
    },
    '&.cm-focused .cm-selectionBackground, .cm-selectionBackground': {
      backgroundColor: 'rgba(91, 140, 255, 0.30)',
    },
    '.cm-gutters': {
      backgroundColor: 'var(--aic-editor-bg)',
      borderRight: '1px solid var(--aic-border)',
      color: 'var(--aic-text-dim)',
    },
    '.cm-lineNumbers .cm-gutterElement': {
      padding: '0 8px 0 12px',
    },
    '.cm-aic-fold-widget': {
      display: 'inline-block',
      margin: '0 4px',
      padding: '0 8px',
      borderRadius: '8px',
      background: 'var(--aic-bg-raised)',
      border: '1px solid var(--aic-border)',
      color: 'var(--aic-text-dim)',
      fontSize: '11px',
      cursor: 'pointer',
      lineHeight: '1.6',
    },
    '.cm-aic-fold-widget:hover': {
      color: 'var(--aic-text)',
      borderColor: 'var(--aic-accent)',
    },
  },
  { dark: true }
);

/**
 * 暗色语法高亮主题（FR-TXT-013/014 简化版）：
 * 色板语义对齐 theme/palette.ts 暗色 token（CSS 变量优先）。
 * 差异行上抑制语法高亮（BC 默认）属 P5 后续微调项。
 */
export const aicHighlightStyle = HighlightStyle.define([
  { tag: t.comment, color: 'var(--aic-text-dim)', fontStyle: 'italic' },
  { tag: [t.keyword, t.modifier, t.operatorKeyword, t.controlKeyword, t.moduleKeyword], color: '#c792ea' },
  { tag: [t.string, t.special(t.string)], color: '#98c379' },
  { tag: [t.number, t.bool, t.null, t.atom], color: '#d19a66' },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: '#61afef' },
  { tag: [t.typeName, t.className, t.namespace], color: '#e5c07b' },
  { tag: [t.propertyName, t.attributeName], color: '#7ec8ff' },
  { tag: [t.variableName, t.labelName], color: 'var(--aic-editor-text)' },
  { tag: [t.definition(t.variableName), t.definition(t.propertyName)], color: '#7ec8ff' },
  { tag: [t.operator, t.punctuation, t.separator, t.bracket], color: 'var(--aic-text)' },
  { tag: t.invalid, color: 'var(--aic-danger)' },
  { tag: [t.heading], color: 'var(--aic-accent)', fontWeight: '600' },
  { tag: [t.link, t.url], color: '#61afef', textDecoration: 'underline' },
  { tag: [t.emphasis], fontStyle: 'italic' },
  { tag: [t.strong], fontWeight: '700' },
  { tag: t.meta, color: 'var(--aic-text-dim)' },
]);
