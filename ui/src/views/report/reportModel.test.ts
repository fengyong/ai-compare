import { describe, expect, it } from 'vitest';
import type { Tab } from '../../shell/store';
import {
  DEFAULT_DIR_COLUMNS,
  LAYOUTS_BY_KIND,
  REPORT_CHUNK,
  REPORT_TARGET_LABELS,
  buildReportParams,
  defaultReportForm,
  escapeHtml,
  isHtmlTarget,
  reportHtmlBody,
  reportFileName,
  sessionIdOfTab,
  wrapReportText,
  type ReportKind,
} from './reportModel';

const tab = (kind: string, sessionId?: string) =>
  ({ kind, [kind]: sessionId ? { sessionId } : undefined } as unknown as Tab);

describe('报告布局与表单（FR-RPT-002）', () => {
  it('除 home 外每类会话都有可用布局，且文本/文件夹布局含文档要求项', () => {
    for (const kind of Object.keys(LAYOUTS_BY_KIND) as ReportKind[]) {
      if (kind === 'home') expect(LAYOUTS_BY_KIND[kind]).toHaveLength(0);
      else expect(LAYOUTS_BY_KIND[kind].length).toBeGreaterThan(0);
    }
    expect(LAYOUTS_BY_KIND.text).toContain('patch');
    expect(LAYOUTS_BY_KIND.text).toContain('interleaved');
    expect(LAYOUTS_BY_KIND.folder).toContain('sideBySide');
    expect(LAYOUTS_BY_KIND.folder).toContain('xml');
    expect(new Set(Object.keys(REPORT_TARGET_LABELS)).size).toBeGreaterThanOrEqual(7);
  });

  it('默认表单取该类型首个布局，文件夹默认列 = size + timestamp', () => {
    expect(defaultReportForm('text').layout).toBe(LAYOUTS_BY_KIND.text[0]);
    expect(defaultReportForm('folder').columns).toEqual(DEFAULT_DIR_COLUMNS);
    expect(DEFAULT_DIR_COLUMNS.size).toBe(true);
    expect(DEFAULT_DIR_COLUMNS.timestamp).toBe(true);
    expect(DEFAULT_DIR_COLUMNS.crc).toBe(false);
  });

  it('buildReportParams 映射全部选项，空标题省略，列仅目录类携带', () => {
    const text = defaultReportForm('text');
    text.title = '  我的报告  ';
    text.lineNumbers = true;
    text.patchStyle = 'context';
    text.contextLines = -3;
    text.wrapColumns = 0;
    const p = buildReportParams(text, 't-1', 'text');
    expect(p).toMatchObject({
      sessionId: 't-1',
      title: '我的报告',
      lineNumbers: true,
      patchStyle: 'context',
      contextLines: 0,
      wrapColumns: 1,
    });
    expect(p.columns).toBeUndefined();
    const blank = buildReportParams(defaultReportForm('text'), 't-2', 'text');
    expect(blank.title).toBeUndefined();
    const unsafe = defaultReportForm('text');
    unsafe.title = '  <script>alert("x")</script>  ';
    expect(buildReportParams(unsafe, 't-3', 'text').title).not.toMatch(/[<>&"']/);
    const folder = buildReportParams(defaultReportForm('folder'), 'd-1', 'folder');
    expect(folder.columns).toEqual(DEFAULT_DIR_COLUMNS);
  });
});

describe('报告换行 / 文件名 / HTML 判定（FR-RPT-001）', () => {
  it('HTML 布局把 character 换行归一为 word，其它布局不受影响', () => {
    const f = defaultReportForm('text');
    f.layout = 'html';
    f.wrap = 'character';
    expect(buildReportParams(f, 't-1', 'text').wrap).toBe('word');
    f.layout = 'summary';
    expect(buildReportParams(f, 't-2', 'text').wrap).toBe('character');
    f.layout = 'html';
    f.wrap = 'none';
    expect(buildReportParams(f, 't-3', 'text').wrap).toBe('none');
  });
  it('HTML 转义与包装：非 html 布局保留换行并转义标签', () => {
    expect(escapeHtml('<a & "b">')).toBe('&lt;a &amp; &quot;b&quot;&gt;');
    const body = reportHtmlBody('a<b\nsecond', 'summary');
    expect(body).toContain('<pre>');
    expect(body).toContain('a&lt;b\nsecond');
    expect(reportHtmlBody('<html>raw</html>', 'html')).toBe('<html>raw</html>');
  });

  it('wrap-none 原样；character 按列硬折；word 按词折', () => {
    const src = 'alpha beta gamma\nshort';
    expect(wrapReportText(src, 'none', 5)).toBe(src);
    expect(wrapReportText('abcdefgh', 'character', 3)).toBe('abc\ndef\ngh');
    expect(wrapReportText('alpha beta gamma', 'word', 10)).toBe('alpha beta\ngamma');
    expect(wrapReportText('x', 'character', 0)).toBe('x');
  });

  it('文件名按目标与布局取扩展名；HTML 目标判定', () => {
    expect(reportFileName('textFile', 'summary')).toMatch(/\.txt$/);
    expect(reportFileName('textFile', 'csv')).toMatch(/\.csv$/);
    expect(reportFileName('htmlFile', 'summary')).toMatch(/\.html$/);
    expect(reportFileName('textFile', 'xml')).toMatch(/\.xml$/);
    expect(isHtmlTarget('htmlClipboard', 'summary')).toBe(true);
    expect(isHtmlTarget('printer', 'summary')).toBe(true);
    expect(isHtmlTarget('textFile', 'html')).toBe(true);
    expect(isHtmlTarget('textFile', 'summary')).toBe(false);
    expect(REPORT_CHUNK).toBeGreaterThan(0);
  });
});

describe('从标签提取会话句柄', () => {
  it('八类会话均可取到 sessionId，home 返回 null', () => {
    for (const kind of ['folder', 'text', 'sync', 'merge', 'merge3', 'picture', 'table', 'hex']) {
      expect(sessionIdOfTab(tab(kind, `${kind}-1`))).toBe(`${kind}-1`);
    }
    expect(sessionIdOfTab(tab('home'))).toBeNull();
    expect(sessionIdOfTab({ kind: 'text' } as unknown as Tab)).toBeNull();
  });
});
