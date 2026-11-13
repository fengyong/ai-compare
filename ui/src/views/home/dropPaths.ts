/**
 * 拖放启动解析（FR-HOME-001：拖入文件/文件夹到主页开始比较；拖到类型大按钮
 * 即以该类型打开）。纯函数层 + DOM 提取层：
 * - extractPathsFromDataTransfer：浏览器 DataTransfer → 路径列表（异步）；
 * - pickTwoPaths：路径列表 → 左右两侧（>2 取前两个；1 个给单侧提示）。
 *
 * 路径来源优先级：text/plain（文件管理器拖出常带路径或 file:// URI）→
 * webkitGetAsEntry 的相对路径（浏览器内拖入本地文件，Tauri 内由
 * Webview.onDragDropEvent 提供绝对路径，接线属 P13 Shell 阶段）。
 */

export interface DragPathSource {
  /** dataTransfer.getData('text/plain') 原文（可多行） */
  text?: string;
  /** dataTransfer.getData('text/uri-list') 原文 */
  uriList?: string;
  /** webkitGetAsEntry 提取的相对路径（浏览器） */
  entryPaths?: string[];
}

/** file:// URI → 本地路径（decodeURIComponent 处理空格/中文） */
export function fileUriToPath(uri: string): string | null {
  if (!uri.startsWith('file://')) return null;
  try {
    let path = decodeURIComponent(uri.slice('file://'.length));
    // file:///path → /path；Windows file:///C:/... → C:/...
    if (path.startsWith('/') && /^\/[A-Za-z]:\//.test(path)) path = path.slice(1);
    return path;
  } catch {
    return null;
  }
}

/** DataTransfer 数据 → 路径列表（去重、去空、file:// 解码） */
export function extractPathsFromSource(src: DragPathSource): string[] {
  const out: string[] = [];
  const push = (p: string | null | undefined) => {
    if (!p) return;
    const t = p.trim();
    if (t && !out.includes(t)) out.push(t);
  };
  if (src.uriList) {
    for (const line of src.uriList.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      push(fileUriToPath(trimmed) ?? trimmed);
    }
  }
  if (out.length === 0 && src.text) {
    for (const line of src.text.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      push(fileUriToPath(trimmed) ?? trimmed);
    }
  }
  if (out.length === 0 && src.entryPaths) {
    for (const p of src.entryPaths) push(p);
  }
  return out;
}

export interface PickedPair {
  left: string | null;
  right: string | null;
  /** 供 UI 提示（如只拖入 1 个路径） */
  warning?: string;
}

/** 路径列表 → 左右对（前两个；其余忽略） */
export function pickTwoPaths(paths: string[]): PickedPair {
  if (paths.length === 0) return { left: null, right: null, warning: '未识别到拖入的路径' };
  if (paths.length === 1) return { left: paths[0], right: null, warning: '只识别到一个路径，请再拖入另一侧' };
  return { left: paths[0], right: paths[1] };
}

/** DOM DataTransfer → 路径列表（items 优先；回退 getData） */
export async function extractPathsFromDataTransfer(dt: DataTransfer): Promise<string[]> {
  const entryPaths: string[] = [];
  const items = dt.items ? Array.from(dt.items) : [];
  for (const item of items) {
    if (item.kind !== 'file') continue;
    const getter = (item as DataTransferItem & { webkitGetAsEntry?: () => FileSystemEntry | null }).webkitGetAsEntry;
    const entry = typeof getter === 'function' ? getter.call(item) : null;
    if (entry) {
      const p = entryFullPath(entry);
      if (p) entryPaths.push(p);
    }
  }
  return extractPathsFromSource({
    text: safeGet(dt, 'text/plain'),
    uriList: safeGet(dt, 'text/uri-list'),
    entryPaths,
  });
}

function safeGet(dt: DataTransfer, type: string): string | undefined {
  try {
    const v = dt.getData(type);
    return v === '' ? undefined : v;
  } catch {
    return undefined;
  }
}

function entryFullPath(entry: FileSystemEntry): string | null {
  const withPath = entry as FileSystemEntry & { fullPath?: string };
  if (typeof withPath.fullPath === 'string' && withPath.fullPath !== '') return withPath.fullPath;
  return entry.name;
}
