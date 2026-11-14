import type { CSSProperties } from 'react';

/**
 * P10 设置两页的内联样式（settings/** 权限边界内的集中样式位；
 * 类名（aic-formats-* / aic-rules-*）保留为主题层后续接管挂钩）。
 * 颜色取主题语义变量（theme/palette 体系），缺省值兜底。
 */

export const pageHint: CSSProperties = { fontSize: 12, color: 'var(--aic-text-dim, #888)' };

export const formatsTable: CSSProperties = {
  width: '100%',
  borderCollapse: 'collapse',
  fontSize: 12,
  marginTop: 8,
};

export const formatsTh: CSSProperties = {
  textAlign: 'left',
  color: 'var(--aic-text-dim, #888)',
  fontWeight: 600,
  padding: '4px 6px',
  borderBottom: '1px solid var(--aic-border, #ccc)',
};

export const formatsTd: CSSProperties = {
  padding: '4px 6px',
  borderBottom: '1px solid var(--aic-border, #ccc)',
  verticalAlign: 'middle',
};

export const disabledRow: CSSProperties = { opacity: 0.45 };

export const shadowedMark: CSSProperties = {
  color: 'var(--aic-danger, #d9534f)',
  fontWeight: 600,
};

export const editBtn: CSSProperties = { marginLeft: 8, fontSize: 11 };

export const elementsCell: CSSProperties = { color: 'var(--aic-text-dim, #888)', maxWidth: 220 };

export const rowActions: CSSProperties = { display: 'flex', gap: 10, marginTop: 10 };

export const editorCell: CSSProperties = { background: 'var(--aic-border, rgba(128,128,128,0.15))' };

export const grammarRow: CSSProperties = { display: 'flex', gap: 8, padding: '2px 0' };

export const packArea: CSSProperties = { display: 'flex', flexDirection: 'column', gap: 8, marginTop: 6 };

export const packTextarea: CSSProperties = { width: '100%', fontFamily: 'monospace', fontSize: 11 };

export const packButtons: CSSProperties = { display: 'flex', gap: 10 };

export const messageLine: CSSProperties = { fontSize: 12, color: 'var(--aic-accent, #4a90d9)' };

export const rulesFlags: CSSProperties = { display: 'flex', flexDirection: 'column', gap: 6, marginTop: 8 };

export const rulesFlag: CSSProperties = { display: 'flex', alignItems: 'center', gap: 8, fontSize: 13 };

export const rulesFormatRow: CSSProperties = { display: 'flex', alignItems: 'center', gap: 10, margin: '8px 0' };

export const rulesElements: CSSProperties = { display: 'flex', flexWrap: 'wrap', gap: '6px 18px', margin: '6px 0' };
