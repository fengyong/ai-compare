/**
 * 单窗格可编辑文本宿主（CM6，D2 / FR-EDIT-001）。
 *
 * 与 `views/text-compare/AlignedPane` 的区别：这里是**真正的单文件编辑器**，
 * 没有对齐行/填充行/差异段概念，文档就是文件内容本身；撤销栈用 CM 自带的
 * history（跨侧撤销在单文件场景没有意义，也不需要模型级撤销）。
 *
 * 只做 DOM 侧装配与少量原语（选区、替换、换行开关、只读开关、语言扩展），
 * 内容真值与引擎同步由 editModel + 视图层负责。
 */

import { Compartment, EditorState, type Extension } from '@codemirror/state';
import {
  EditorView,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from '@codemirror/view';
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  redo,
  undo,
} from '@codemirror/commands';
import {
  bracketMatching,
  defaultHighlightStyle,
  indentOnInput,
  syntaxHighlighting,
} from '@codemirror/language';

export interface EditablePaneOptions {
  /** 初始文档文本（= lines.join('\n')） */
  docText: string;
  /** 用户编辑回调（programmatic setText 不触发） */
  onChange(text: string): void;
  /** 光标/选区变化（状态栏行列） */
  onCursor?(line: number, col: number): void;
  readOnly?: boolean;
  wrap?: boolean;
}

/** 文档内容变化回调的抑制开关（程序化回灌不回声给同步层） */
interface PaneState {
  suppress: boolean;
}

export class EditablePane {
  readonly view: EditorView;
  private readonly state: PaneState = { suppress: false };
  private readonly readOnlyComp = new Compartment();
  private readonly wrapComp = new Compartment();
  private readonly langComp = new Compartment();
  private readonly opts: EditablePaneOptions;

  private constructor(parent: HTMLElement, opts: EditablePaneOptions) {
    this.opts = opts;
    const updateListener = EditorView.updateListener.of((u) => {
      if (u.docChanged && !this.state.suppress) this.opts.onChange(u.state.doc.toString());
      if (u.selectionSet || u.docChanged) {
        const head = u.state.selection.main.head;
        const line = u.state.doc.lineAt(head);
        this.opts.onCursor?.(line.number - 1, head - line.from);
      }
    });
    this.view = new EditorView({
      parent,
      state: EditorState.create({
        doc: opts.docText,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightSpecialChars(),
          history(),
          drawSelection(),
          dropCursor(),
          EditorState.allowMultipleSelections.of(true),
          indentOnInput(),
          syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
          bracketMatching(),
          rectangularSelection(),
          highlightActiveLine(),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          updateListener,
          this.readOnlyComp.of(this.readOnlyExtensions(!!opts.readOnly)),
          this.wrapComp.of(opts.wrap === false ? [] : EditorView.lineWrapping),
          this.langComp.of([]),
        ],
      }),
    });
  }

  static create(parent: HTMLElement, opts: EditablePaneOptions): EditablePane {
    return new EditablePane(parent, opts);
  }

  private readOnlyExtensions(readOnly: boolean): Extension {
    return [EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)];
  }

  getText(): string {
    return this.view.state.doc.toString();
  }

  /** 程序化替换全文（保留撤销历史的边界：作为一次 transaction 落账） */
  setText(text: string): void {
    if (this.getText() === text) return;
    this.state.suppress = true;
    try {
      this.view.dispatch({
        changes: { from: 0, to: this.view.state.doc.length, insert: text },
      });
    } finally {
      this.state.suppress = false;
    }
  }

  /** 以字符区间替换（查找替换/局部修改用；进入 CM 撤销栈并由 onChange 上报） */
  replaceRange(from: number, to: number, insert: string): void {
    this.view.dispatch({ changes: { from, to, insert } });
  }

  /** 选中区间并滚入视野（查找导航） */
  selectRange(from: number, to: number): void {
    this.view.dispatch({
      selection: { anchor: from, head: to },
      scrollIntoView: true,
    });
    this.view.focus();
  }

  cursor(): { line: number; col: number } {
    const head = this.view.state.selection.main.head;
    const line = this.view.state.doc.lineAt(head);
    return { line: line.number - 1, col: head - line.from };
  }

  setReadOnly(readOnly: boolean): void {
    this.view.dispatch({
      effects: this.readOnlyComp.reconfigure(this.readOnlyExtensions(readOnly)),
    });
  }

  setWrap(wrap: boolean): void {
    this.view.dispatch({
      effects: this.wrapComp.reconfigure(wrap ? EditorView.lineWrapping : []),
    });
  }

  setLanguage(ext: Extension | null): void {
    this.view.dispatch({ effects: this.langComp.reconfigure(ext ? [ext] : []) });
  }

  undo(): void {
    undo(this.view);
  }

  redo(): void {
    redo(this.view);
  }

  focus(): void {
    this.view.focus();
  }

  destroy(): void {
    this.view.destroy();
  }
}
