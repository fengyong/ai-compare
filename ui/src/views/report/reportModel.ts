import type { ReportParams } from '../../ipc/engineProcess';
import type {
  ReportColorMode,
  ReportDirColumns,
  ReportDirDisplayFilter,
  ReportDisplayFilter,
  ReportLayout,
  ReportPatchStyle,
  ReportWrapMode,
} from '../../protocol/types';
import type { Tab } from '../../shell/store';

/**
 * 报告对话框模型（docs/01 §4.7 FR-RPT-001/002；消费 P11 report/generate）。
 * 纯函数集中在此便于测试；副作用（下载/剪贴板/打印/浏览器）也在本模块，
 * 组件只负责表单与预览。
 */

export type ReportKind = Tab['kind'];

/** FR-RPT-002 各会话类型可用布局（引擎布局族的 UI 子集） */
export const LAYOUTS_BY_KIND: Record<ReportKind, readonly ReportLayout[]> = {
  home: [],
  text: ['sideBySide', 'summary', 'interleaved', 'patch', 'csv', 'xml', 'html', 'stats'],
  table: ['sideBySide', 'summary', 'interleaved', 'csv', 'xml', 'html', 'stats'],
  hex: ['sideBySide', 'summary', 'interleaved', 'xml', 'html', 'stats'],
  folder: ['sideBySide', 'summary', 'xml', 'tree', 'list', 'stats', 'html'],
  sync: ['summary', 'list', 'stats', 'html'],
  merge: ['summary', 'list', 'stats', 'html'],
  merge3: ['summary', 'sideBySide', 'html', 'xml', 'stats'],
  picture: ['sideBySide', 'summary', 'html'],
};

export const REPORT_LAYOUT_LABELS: Record<ReportLayout, string> = {
  sideBySide: '并排（side-by-side）',
  summary: '摘要（summary）',
  interleaved: '交错（interleaved）',
  patch: '补丁（patch）',
  html: 'HTML',
  xml: 'XML',
  csv: '统计表（CSV）',
  tree: '目录树（tree）',
  list: '清单（list）',
  stats: '统计（stats）',
};

/** FR-RPT-001 输出目标：打印机 / HTML 文件或剪贴板 / 纯文本文件或剪贴板 + 浏览器查看 */
export type ReportTarget =
  | 'preview'
  | 'printer'
  | 'htmlFile'
  | 'htmlClipboard'
  | 'textFile'
  | 'textClipboard'
  | 'browser';

export const REPORT_TARGET_LABELS: Record<ReportTarget, string> = {
  preview: '仅预览',
  printer: '打印预览（打印机）',
  htmlFile: 'HTML 文件',
  htmlClipboard: 'HTML → 剪贴板',
  textFile: '纯文本文件',
  textClipboard: '纯文本 → 剪贴板',
  browser: '在浏览器中查看',
};

export interface ReportForm {
  layout: ReportLayout;
  title: string;
  display: ReportDisplayFilter;
  dirDisplay: ReportDirDisplayFilter;
  ignoreUnimportant: boolean;
  lineNumbers: boolean;
  contextLines: number;
  patchStyle: ReportPatchStyle;
  delimiter: 'csv' | 'tsv';
  color: ReportColorMode;
  wrap: ReportWrapMode;
  wrapColumns: number;
  includeFileLinks: boolean;
  columns: ReportDirColumns;
}

export const DEFAULT_DIR_COLUMNS: ReportDirColumns = {
  size: true,
  timestamp: true,
  attributes: false,
  crc: false,
  vcs: false,
  revision: false,
  version: false,
  owner: false,
  group: false,
};

export function defaultReportForm(kind: ReportKind): ReportForm {
  return {
    layout: LAYOUTS_BY_KIND[kind][0] ?? 'summary',
    title: '',
    display: 'all',
    dirDisplay: 'all',
    ignoreUnimportant: false,
    lineNumbers: false,
    contextLines: 3,
    patchStyle: 'unified',
    delimiter: 'csv',
    color: 'color',
    wrap: 'none',
    wrapColumns: 80,
    includeFileLinks: false,
    columns: { ...DEFAULT_DIR_COLUMNS },
  };
}

/** 表单 → report/generate 参数（引擎忽略不适用的布局级选项） */
export function buildReportParams(form: ReportForm, sessionId: string, kind: ReportKind): ReportParams {
  const p: ReportParams = { sessionId, layout: form.layout };
  // 标题会进入 HTML 报告（layout=html）与打印窗口，去掉 HTML 敏感字符防止注入
  if (form.title.trim()) p.title = form.title.trim().replace(/[&<>"']/g, '');
  p.display = form.display;
  p.dirDisplay = form.dirDisplay;
  p.ignoreUnimportant = form.ignoreUnimportant;
  p.lineNumbers = form.lineNumbers;
  p.contextLines = Math.max(0, Math.trunc(form.contextLines));
  p.patchStyle = form.patchStyle;
  p.delimiter = form.delimiter;
  p.color = form.color;
  // FR-RPT-001：HTML 仅支持 wrap-none / wrap-word，character 归一到 word
  p.wrap = form.layout === 'html' && form.wrap === 'character' ? 'word' : form.wrap;
  p.wrapColumns = Math.max(1, Math.trunc(form.wrapColumns));
  p.includeFileLinks = form.includeFileLinks;
  if (kind === 'folder' || kind === 'sync' || kind === 'merge') p.columns = { ...form.columns };
  return p;
}

/** FR-RPT-001 换行：HTML 仅 none/word，character 为字符硬折行 */
export function wrapReportText(text: string, wrap: ReportWrapMode, columns: number): string {
  if (wrap === 'none' || columns <= 0) return text;
  return text
    .split('\n')
    .map((line) => {
      if (wrap === 'character') {
        const rows: string[] = [];
        for (let i = 0; i < line.length; i += columns) rows.push(line.slice(i, i + columns));
        return rows.length > 0 ? rows.join('\n') : '';
      }
      const rows: string[] = [];
      let cur = '';
      for (const word of line.split(' ')) {
        if (cur.length === 0) cur = word;
        else if (cur.length + 1 + word.length <= columns) cur += ` ${word}`;
        else {
          rows.push(cur);
          cur = word;
        }
      }
      if (cur.length > 0) rows.push(cur);
      return rows.join('\n');
    })
    .join('\n');
}

export function reportFileName(target: ReportTarget, layout: ReportLayout): string {
  const ext = target === 'htmlFile' || target === 'htmlClipboard' || layout === 'html' ? 'html'
    : layout === 'csv' ? 'csv'
    : layout === 'xml' ? 'xml'
    : layout === 'patch' ? 'patch'
    : 'txt';
  return `ai-compare-report-${layout}.${ext}`;
}

export function isHtmlTarget(target: ReportTarget, layout: ReportLayout): boolean {
  return target === 'htmlFile' || target === 'htmlClipboard' || target === 'printer' || target === 'browser' || layout === 'html';
}

/** 从任意标签取引擎会话句柄（各 TabData 均有 sessionId） */
export function sessionIdOfTab(tab: Tab): string | null {
  const keys: Partial<Record<ReportKind, string>> = {
    folder: 'folder',
    text: 'text',
    sync: 'sync',
    merge: 'merge',
    merge3: 'merge3',
    picture: 'picture',
    table: 'table',
    hex: 'hex',
  };
  const key = keys[tab.kind];
  if (!key) return null;
  const data = (tab as unknown as Record<string, { sessionId?: string } | undefined>)[key];
  return data?.sessionId ?? null;
}

/** 报告分块大小（字符数；引擎按 Unicode 标量切块） */
export const REPORT_CHUNK = 64 * 1024;

/** HTML 输出用转义（防止报告内容里的 <、& 被当标签解析） */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/**
 * 非 html 布局却选择 HTML 输出目标时，包装成合法 HTML 并用 <pre> 保留换行；
 * layout=html 时引擎已产出完整 HTML，原样输出。
 */
export function reportHtmlBody(text: string, layout: ReportLayout): string {
  if (layout === 'html') return text;
  return `<!doctype html><html><head><meta charset="utf-8"></head><body><pre>${escapeHtml(text)}</pre></body></html>`;
}

/** FR-RPT-001 输出目标落地 */
export async function deliverReport(
  text: string,
  target: ReportTarget,
  layout: ReportLayout,
  notice: (msg: string) => void
): Promise<void> {
  if (target === 'preview') return;
  if (target === 'textClipboard' || target === 'htmlClipboard') {
    try {
      await navigator.clipboard.writeText(text);
      notice('报告已复制到剪贴板');
    } catch {
      notice('剪贴板不可用，请改用文件输出');
    }
    return;
  }
  if (target === 'textFile' || target === 'htmlFile') {
    const isHtmlFile = target === 'htmlFile';
    const blob = new Blob([isHtmlFile ? reportHtmlBody(text, layout) : text], {
      type: isHtmlFile ? 'text/html;charset=utf-8' : 'text/plain;charset=utf-8',
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = reportFileName(target, layout);
    a.click();
    URL.revokeObjectURL(url);
    notice('报告已导出为文件');
    return;
  }
  if (target === 'browser' || target === 'printer') {
    if (typeof window === 'undefined') return;
    const w = window.open('', '_blank');
    if (!w) {
      notice('浏览器拦截了新窗口，请允许弹窗后重试');
      return;
    }
    w.document.write(reportHtmlBody(text, layout));
    w.document.close();
    if (target === 'printer') w.print();
    notice(target === 'printer' ? '已打开打印预览' : '已在浏览器中查看报告');
  }
}
