import { EditorState, type Extension, type Range } from '@codemirror/state';
import { Decoration, EditorView, GutterMarker, gutter, lineNumbers } from '@codemirror/view';
import type { AlignedRow } from './alignment';
import type { InlineMark } from '../../lib/inlineDiffCore';

/**
 * CM6 "对齐层"装饰（docs/03 §3.4.3 M32，ADR-008：CM6 为编辑内核、差异布局自研）。
 *
 * P0 为只读视图：文档与装饰一次性静态构建（EditorView.decorations 静态注入）；
 * 编辑态的增量装饰更新（transaction 驱动 StateField）在 P5 编辑链路时引入。
 *
 * 装饰语义（docs/01 附录 C.3）：
 * - 重要差异行：浅红底纹（cm-aic-line-diff），浅红优先于浅蓝
 * - 不重要差异行：浅蓝底纹（cm-aic-line-insig）
 * - 对侧缺失的填充行：更弱底纹（cm-aic-line-absent）+ gutter 空心点
 * - 行内 span 级差异：Mark 高亮（cm-aic-inline-diff）
 * - gutter 彩点标记每行整体状态（红/蓝/空心）
 */

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

function lineClassFor(status: AlignedRow['status'], importance: 'important' | 'unimportant'): string | null {
  switch (status) {
    case 'equal':
      return null;
    case 'absent':
      return 'cm-aic-line-absent';
    case 'inserted':
    case 'deleted':
    case 'changed':
      return importance === 'important' ? 'cm-aic-line-diff' : 'cm-aic-line-insig';
  }
}

function gutterMarkerFor(row: AlignedRow, importance: 'important' | 'unimportant'): GutterMarker | null {
  switch (row.status) {
    case 'equal':
      return null;
    case 'absent':
      return MARKER_ABSENT;
    case 'inserted':
    case 'deleted':
    case 'changed':
      return importance === 'important' ? MARKER_IMPORTANT : MARKER_UNIMPORTANT;
  }
}

export interface PaneBuildInput {
  /** 对齐文档文本（AlignedDoc.lines.join('\n')） */
  docText: string;
  rows: AlignedRow[];
  /** 每行差异的重要性（P0 全部 important；随引擎 section.importance 传入） */
  sectionImportance: Map<number, 'important' | 'unimportant' | 'ignored'>;
  /** 行内高亮 span（0 基行号 → 字符区间） */
  marks: InlineMark[];
}

/** 构建单侧窗格的全部 CM6 扩展（行底纹 + 行内 mark + gutter 彩点 + 主题/只读） */
export function buildPaneExtensions(input: PaneBuildInput): Extension[] {
  const doc = EditorState.create({ doc: input.docText, extensions: [] }).doc;
  const ranges: Array<Range<Decoration>> = [];

  // 行级装饰 + gutter 数据
  for (let i = 0; i < input.rows.length; i++) {
    const row = input.rows[i];
    const importanceRaw = input.sectionImportance.get(row.sectionId) ?? 'important';
    const importance = importanceRaw === 'unimportant' ? 'unimportant' : 'important';
    const line = doc.line(i + 1);
    const cls = lineClassFor(row.status, importance);
    if (cls) {
      ranges.push(Decoration.line({ class: cls }).range(line.from));
    }
  }
  // 行内 mark 装饰
  for (const mk of input.marks) {
    if (mk.line + 1 > doc.lines) continue;
    const line = doc.line(mk.line + 1);
    const from = Math.min(line.from + mk.from, line.to);
    const to = Math.min(line.from + mk.to, line.to);
    if (to > from) {
      ranges.push(Decoration.mark({ class: 'cm-aic-inline-diff' }).range(from, to));
    }
  }

  const decorationSet = Decoration.set(ranges, true);

  const statusGutter = gutter({
    class: 'cm-aic-status-gutter',
    lineMarker: (view, line) => {
      const lineNumber = view.state.doc.lineAt(line.from).number;
      const row = input.rows[lineNumber - 1];
      if (!row) return null;
      const importanceRaw = input.sectionImportance.get(row.sectionId) ?? 'important';
      const importance = importanceRaw === 'unimportant' ? 'unimportant' : 'important';
      return gutterMarkerFor(row, importance);
    },
  });

  return [
    lineNumbers(),
    statusGutter,
    EditorView.decorations.of(decorationSet),
    editorTheme,
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
  ];
}

/** 编辑器暗色主题（颜色引用 palette 注入的 CSS 变量） */
const editorTheme = EditorView.theme(
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
      caretColor: 'transparent',
    },
    '.cm-gutters': {
      backgroundColor: 'var(--aic-editor-bg)',
      borderRight: '1px solid var(--aic-border)',
      color: 'var(--aic-text-dim)',
    },
    '.cm-lineNumbers .cm-gutterElement': {
      padding: '0 8px 0 12px',
    },
  },
  { dark: true }
);
