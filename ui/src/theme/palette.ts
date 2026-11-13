/**
 * 色板集中管理（暗色优先）。
 *
 * 差异配色语义对齐 docs/01 附录 C（BC 惯例），docs/03 ADR-010（颜色默认值对齐 BC）：
 * - 黑=相同（暗色主题映射为正常前景色）
 * - 灰=未知/较旧
 * - 紫=孤立（另一侧不存在）
 * - 红=较新/不同（重要差异），行底纹=浅红
 * - 蓝=不重要差异，行底纹=浅蓝（浅红优先于浅蓝）
 * - 青绿=被过滤器隐藏（抑制过滤器时显示）
 * - 黄=文件夹未扫描
 *
 * 所有 token 以 CSS 自定义属性（--aic-*）注入 :root，组件层只引用变量。
 * 主题切换/浅色主题在 P13（docs/04）扩展，届时仅替换本文件的 token 值。
 */

export interface ThemeTokens {
  /* 基础 */
  bg: string;
  bgPanel: string;
  bgRaised: string;
  border: string;
  text: string;
  textDim: string;
  accent: string;
  accentSoft: string;
  danger: string;

  /* 目录状态色（附录 C.1） */
  statusSame: string;
  statusOlder: string;
  statusOrphan: string;
  statusDiff: string;
  statusFiltered: string;
  statusUnscanned: string;
  statusError: string;

  /* 行底纹（附录 C.3 文本比较：浅红/浅蓝） */
  rowBgDiff: string;
  rowBgUnimportant: string;
  rowBgAbsent: string;

  /* 文本视图 */
  editorBg: string;
  editorText: string;
  inlineDiffMark: string;
  gutterImportant: string;
  gutterUnimportant: string;
  gutterAbsent: string;

  /* 概览条（FR-TXT-030 mini-map 色带） */
  overviewEqual: string;
  overviewDiff: string;
  overviewUnimportant: string;
  overviewAbsent: string;
}

export const darkTokens: ThemeTokens = {
  bg: '#1b1d21',
  bgPanel: '#23262b',
  bgRaised: '#2b2f36',
  border: '#363b44',
  text: '#d7dae0',
  textDim: '#8b919c',
  accent: '#5b8cff',
  accentSoft: 'rgba(91, 140, 255, 0.16)',
  danger: '#e5534b',

  statusSame: '#d7dae0',
  statusOlder: '#8b919c',
  statusOrphan: '#b57edc',
  statusDiff: '#ff6b6b',
  statusFiltered: '#2fbfa5',
  statusUnscanned: '#e5c07b',
  statusError: '#ff6b6b',

  rowBgDiff: 'rgba(255, 107, 107, 0.13)',
  rowBgUnimportant: 'rgba(96, 165, 250, 0.14)',
  rowBgAbsent: 'rgba(255, 255, 255, 0.028)',

  editorBg: '#1e2126',
  editorText: '#d7dae0',
  inlineDiffMark: 'rgba(255, 107, 107, 0.32)',
  gutterImportant: '#ff6b6b',
  gutterUnimportant: '#60a5fa',
  gutterAbsent: '#565b64',

  overviewEqual: 'rgba(215, 218, 224, 0.16)',
  overviewDiff: '#ff6b6b',
  overviewUnimportant: '#60a5fa',
  overviewAbsent: 'rgba(255, 255, 255, 0.05)',
};

export function applyTheme(tokens: ThemeTokens = darkTokens): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  const map: Record<string, string> = {
    'aic-bg': tokens.bg,
    'aic-bg-panel': tokens.bgPanel,
    'aic-bg-raised': tokens.bgRaised,
    'aic-border': tokens.border,
    'aic-text': tokens.text,
    'aic-text-dim': tokens.textDim,
    'aic-accent': tokens.accent,
    'aic-accent-soft': tokens.accentSoft,
    'aic-danger': tokens.danger,
    'aic-st-same': tokens.statusSame,
    'aic-st-older': tokens.statusOlder,
    'aic-st-orphan': tokens.statusOrphan,
    'aic-st-diff': tokens.statusDiff,
    'aic-st-filtered': tokens.statusFiltered,
    'aic-st-unscanned': tokens.statusUnscanned,
    'aic-st-error': tokens.statusError,
    'aic-rowbg-diff': tokens.rowBgDiff,
    'aic-rowbg-unimportant': tokens.rowBgUnimportant,
    'aic-rowbg-absent': tokens.rowBgAbsent,
    'aic-editor-bg': tokens.editorBg,
    'aic-editor-text': tokens.editorText,
    'aic-inline-mark': tokens.inlineDiffMark,
    'aic-gutter-important': tokens.gutterImportant,
    'aic-gutter-unimportant': tokens.gutterUnimportant,
    'aic-gutter-absent': tokens.gutterAbsent,
    'aic-overview-equal': tokens.overviewEqual,
    'aic-overview-diff': tokens.overviewDiff,
    'aic-overview-unimportant': tokens.overviewUnimportant,
    'aic-overview-absent': tokens.overviewAbsent,
  };
  for (const [k, v] of Object.entries(map)) {
    root.style.setProperty(`--${k}`, v);
  }
}
