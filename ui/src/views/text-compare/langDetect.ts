/**
 * 语法高亮语言探测（FR-TXT-013/014 简化版）：按文件扩展名映射 Lezer 语言包。
 * 纯映射层（可单测）；语言包动态加载见 langPacks.ts（CM 层，按需 import）。
 */

export type LangId = 'javascript' | 'typescript' | 'json' | 'markdown' | 'python' | 'rust';

const EXT_MAP: Record<string, LangId> = {
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'javascript',
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  json: 'json',
  jsonc: 'json',
  md: 'markdown',
  markdown: 'markdown',
  py: 'python',
  pyi: 'python',
  rs: 'rust',
};

/** 由文件名取语言 id；无语言包命中返回 null（回退纯文本） */
export function detectLangId(fileName: string): LangId | null {
  const dot = fileName.lastIndexOf('.');
  if (dot < 0 || dot === fileName.length - 1) return null;
  const ext = fileName.slice(dot + 1).toLowerCase();
  return EXT_MAP[ext] ?? null;
}

/** 语言显示名（视图工具栏/状态提示） */
export const LANG_LABELS: Record<LangId, string> = {
  javascript: 'JavaScript',
  typescript: 'TypeScript',
  json: 'JSON',
  markdown: 'Markdown',
  python: 'Python',
  rust: 'Rust',
};
