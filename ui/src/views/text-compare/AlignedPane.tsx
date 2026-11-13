import { useEffect, useRef } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import type { AlignedDoc } from './alignment';
import type { InlineMark } from '../../lib/inlineDiffCore';
import { buildPaneExtensions } from './cmDecorations';

/**
 * 对齐文本窗格：单个 CM6 只读编辑器实例。
 *
 * 生命周期：props 数据（对齐文档 + 装饰输入）就绪后创建 EditorView；
 * 数据变化时整体重建（P0 只读、一次性加载；编辑态增量更新属 P5）。
 * 滚动同步由父组件接管：本组件只把 scrollDOM 的 scroll 事件上报。
 */

export interface AlignedPaneProps {
  head: string;
  doc: AlignedDoc;
  marks: InlineMark[];
  sectionImportance: Map<number, 'important' | 'unimportant' | 'ignored'>;
  onScroll?: (scrollDOM: HTMLElement) => void;
  registerView?: (view: EditorView | null) => void;
}

export function AlignedPane({ head, doc, marks, sectionImportance, onScroll, registerView }: AlignedPaneProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const scrollCbRef = useRef(onScroll);
  scrollCbRef.current = onScroll;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const docText = doc.lines.join('\n');
    const extensions = buildPaneExtensions({
      docText,
      rows: doc.rows,
      sectionImportance,
      marks,
    });
    const view = new EditorView({
      parent: host,
      state: EditorState.create({ doc: docText, extensions }),
    });
    registerView?.(view);

    const handleScroll = () => scrollCbRef.current?.(view.scrollDOM);
    view.scrollDOM.addEventListener('scroll', handleScroll);

    return () => {
      view.scrollDOM.removeEventListener('scroll', handleScroll);
      view.destroy();
      registerView?.(null);
    };
    // 数据一次性构建；doc/marks 身份稳定（由父级 useMemo 保证）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc, marks, sectionImportance]);

  return (
    <div className="aic-pane">
      <div className="aic-pane-head" title={head}>
        {head}
      </div>
      <div className="aic-pane-body" ref={hostRef} />
    </div>
  );
}
