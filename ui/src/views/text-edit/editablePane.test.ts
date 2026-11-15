// @vitest-environment jsdom
/**
 * 单窗格 CM6 宿主（D2）：只验证"编辑器原语"与视图契约的接缝——
 * 内容回调、程序化回灌的抑制、选区/光标坐标、只读与换行开关。
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { EditablePane } from './editablePane';

let host: HTMLDivElement;
let pane: EditablePane | null = null;

function mount(
  docText: string,
  readOnly = false,
): { pane: EditablePane; onChange: ReturnType<typeof vi.fn> } {
  host = document.createElement('div');
  document.body.appendChild(host);
  const onChange = vi.fn();
  pane = EditablePane.create(host, { docText, onChange, readOnly });
  return { pane, onChange };
}

afterEach(() => {
  pane?.destroy();
  pane = null;
  host?.remove();
});

describe('text-edit/editablePane', () => {
  it('承载文档内容，用户编辑触发 onChange', () => {
    const { pane: p, onChange } = mount('a\nb');
    expect(p.getText()).toBe('a\nb');
    p.view.dispatch({ changes: { from: 0, insert: 'X' } });
    expect(p.getText()).toBe('Xa\nb');
    expect(onChange).toHaveBeenCalledWith('Xa\nb');
  });

  it('程序化 setText 不回声给同步层（避免自激同步循环）', () => {
    const { pane: p, onChange } = mount('a');
    p.setText('a\nb');
    expect(p.getText()).toBe('a\nb');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('replaceRange 进入撤销栈并上报（查找替换路径）', () => {
    const { pane: p, onChange } = mount('abc');
    p.replaceRange(1, 2, 'Z');
    expect(p.getText()).toBe('aZc');
    expect(onChange).toHaveBeenCalledWith('aZc');
    p.undo();
    expect(p.getText()).toBe('abc');
    p.redo();
    expect(p.getText()).toBe('aZc');
  });

  it('cursor/selectRange：行号 0 基、列号字符偏移', () => {
    const { pane: p } = mount('aa\nbbb');
    // selectRange 的 head 在 to 端，cursor() 报 head：偏移 6 → 行 1（'bbb' 起于 3）列 3
    p.selectRange(4, 6);
    expect(p.cursor()).toEqual({ line: 1, col: 3 });
    p.selectRange(0, 0);
    expect(p.cursor()).toEqual({ line: 0, col: 0 });
  });

  it('只读开关：读写 editable 状态', () => {
    const { pane: p } = mount('a', true);
    // ContentDOM 的 contentEditable 属性由 EditorView.editable 驱动
    expect(p.view.contentDOM.getAttribute('contenteditable')).toBe('false');
    p.setReadOnly(false);
    expect(p.view.contentDOM.getAttribute('contenteditable')).not.toBe('false');
  });

  it('wrap 开关可切换（不抛错，文档不变）', () => {
    const { pane: p } = mount('a');
    p.setWrap(false);
    p.setWrap(true);
    expect(p.getText()).toBe('a');
  });
});
