/** 运行环境探测与通用小工具 */

/** 是否运行在 Tauri WebView 内（Tauri 2 注入 __TAURI_INTERNALS__） */
export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** 取路径基名（兼容 / 与 \），用于标签页标题 */
export function basename(p: string): string {
  const norm = p.replace(/[\\/]+$/, '');
  const i = Math.max(norm.lastIndexOf('/'), norm.lastIndexOf('\\'));
  return i >= 0 ? norm.slice(i + 1) : norm;
}

/** 基目录 + POSIX 风格相对路径（生成协议 DirRow.id）→ 完整路径 */
export function joinPath(base: string, rel: string): string {
  return `${base.replace(/[\\/]+$/, '')}/${rel}`;
}

/** 是否按文本视图打开（P0 的 UI 侧格式探测近似；正式探测属引擎 FormatRegistry，P2） */
const TEXTUAL_EXTS = new Set([
  'c', 'h', 'cpp', 'hpp', 'cc', 'hh', 'cs', 'go', 'rs', 'java', 'kt', 'swift',
  'py', 'rb', 'php', 'pl', 'sh', 'bash', 'zsh', 'fish',
  'js', 'ts', 'jsx', 'tsx', 'mjs', 'cjs', 'json', 'jsonc', 'json5',
  'html', 'htm', 'css', 'scss', 'less', 'xml', 'svg', 'yml', 'yaml', 'toml', 'ini', 'cfg',
  'md', 'markdown', 'txt', 'log', 'sql', 'lua', 'vim', 'proto', 'gradle', 'properties', 'patch', 'diff',
]);

export function looksTextual(name: string): boolean {
  const i = name.lastIndexOf('.');
  if (i < 0) return true; // 无扩展名按文本处理（Makefile、LICENSE 等）
  return TEXTUAL_EXTS.has(name.slice(i + 1).toLowerCase());
}

/** 简单字符串哈希（FNV-1a 32 位），演示引擎用它做确定性种子 */
export function hashString(s: string): number {
  let h = 2166136261 >>> 0;
  for (const ch of s) {
    h ^= ch.codePointAt(0) ?? 0;
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** 确定性伪随机数（mulberry32），演示引擎保证同样输入产出同样数据 */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** 千分位格式化 */
export function formatCount(n: number): string {
  return n.toLocaleString('zh-CN');
}

/** 文件大小格式化（B/KB/MB/GB） */
export function formatSize(bytes: number | undefined): string {
  if (bytes === undefined) return '';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[u]}`;
}

/** 时间格式化（相对或绝对） */
export function formatTime(ms: number | undefined): string {
  if (ms === undefined) return '';
  const d = new Date(ms);
  const pad = (x: number) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
