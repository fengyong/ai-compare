/**
 * 目录过滤器求值（FR-DIR-031/033/035 的 UI 侧数据面，纯函数）：
 * - 名称过滤器输入栏文本 -> NameFilterSpec（分号分隔 + `-` 排除 + 尾部 `\` 目录掩码，
 *   掩码通配符 BC 子集：? * [seq] [!seq]，默认大小写不敏感——与引擎 mask 语义对齐）；
 * - 显示过滤器求值（状态 -> 类别可见性；QuickDiff 按 mtime 分左右较新，
 *   与引擎 session.rs display_visible 同口径——mock 引擎复用本函数保证两端一致）；
 * - hidden/suppressed 标记重算（抑制过滤器语义：被隐藏行以 suppressed 青绿显示）。
 */

import type {
  DirEntryStatus,
  DirRow,
  DisplayFilterSpec,
  NameFilterSpec,
  SideInfo,
} from '../../protocol/types';

// ------------------------------------------------------------------
// 掩码匹配（BC 通配符子集）
// ------------------------------------------------------------------

/**
 * 单掩码匹配：`?` 恰一字符、`*` 任意（含空）、`[abc]`/`[a-z]` 集合、
 * `[!abc]` 补集、`[[]` 匹配 `[`。末尾句点约束（无扩展名过滤）不在此实现
 * （引擎 filter.rs 全集为准；UI 侧覆盖工具栏常用形态）。
 */
export function wildcardMatch(mask: string, name: string, caseInsensitive = true): boolean {
  const m = caseInsensitive ? mask.toLowerCase() : mask;
  const s = caseInsensitive ? name.toLowerCase() : name;
  return matchHere(m, 0, s, 0);
}

function matchHere(mask: string, mi: number, str: string, si: number): boolean {
  while (mi < mask.length) {
    const mc = mask[mi];
    if (mc === '*') {
      // 连续 * 折叠；尝试任意起点
      let next = mi + 1;
      while (next < mask.length && mask[next] === '*') next++;
      for (let k = si; k <= str.length; k++) {
        if (matchHere(mask, next, str, k)) return true;
      }
      return false;
    }
    if (mc === '?') {
      if (si >= str.length) return false;
      mi++;
      si++;
      continue;
    }
    if (mc === '[') {
      const close = findClassEnd(mask, mi);
      if (close === -1) {
        // 非法类：按字面 '['
        if (si >= str.length || str[si] !== '[') return false;
        mi++;
        si++;
        continue;
      }
      const body = mask.slice(mi + 1, close);
      const negated = body.startsWith('!');
      const chars = negated ? body.slice(1) : body;
      if (si >= str.length) return false;
      const hit = classContains(chars, str[si]);
      if (negated ? hit : !hit) return false;
      mi = close + 1;
      si++;
      continue;
    }
    if (si >= str.length || str[si] !== mc) return false;
    mi++;
    si++;
  }
  return si === str.length;
}

/** 类区间结束下标（处理 `]` 首字符字面与 `[[]` 形态）；无闭合返回 -1 */
function findClassEnd(mask: string, open: number): number {
  for (let i = open + 1; i < mask.length; i++) {
    if (mask[i] === ']' && i > open + 1) return i;
    // `[]...]`：首字符为 ] 视为字面
    if (mask[i] === ']' && i === open + 1) continue;
  }
  return -1;
}

function classContains(chars: string, c: string): boolean {
  let i = 0;
  while (i < chars.length) {
    if (i + 2 < chars.length && chars[i + 1] === '-') {
      if (c >= chars[i] && c <= chars[i + 2]) return true;
      i += 3;
      continue;
    }
    if (chars[i] === c) return true;
    i++;
  }
  return false;
}

// ------------------------------------------------------------------
// 名称过滤器输入栏文本 -> NameFilterSpec
// ------------------------------------------------------------------

/**
 * 解析工具栏过滤输入：分号分隔；`-` 前缀 = 排除；尾部 `\` = 目录掩码。
 * 例：`*.txt; -*.log; -build\` → includeFiles:[*.txt], excludeFiles:[*.log],
 * excludeDirs:[build]。
 */
export function parseMaskInput(text: string, caseSensitive = false): NameFilterSpec {
  const spec: NameFilterSpec = {
    includeFiles: [],
    excludeFiles: [],
    includeDirs: [],
    excludeDirs: [],
    caseSensitive: caseSensitive ? true : null,
  };
  const trimmed = text.trim();
  if (!trimmed) return spec;
  for (const raw of trimmed.split(';')) {
    const item = raw.trim();
    if (!item) continue;
    let negated = false;
    let body = item;
    if (body.startsWith('-')) {
      negated = true;
      body = body.slice(1).trim();
      if (!body) continue;
    }
    const isDir = body.endsWith('\\') || body.endsWith('/');
    if (isDir) body = body.slice(0, -1);
    if (isDir) {
      if (negated) spec.excludeDirs.push(body);
      else spec.includeDirs.push(body);
    } else if (negated) {
      spec.excludeFiles.push(body);
    } else {
      spec.includeFiles.push(body);
    }
  }
  return spec;
}

/** NameFilterSpec -> 工具栏文本（回显；输入与往返可对拍） */
export function maskInputFromSpec(spec: NameFilterSpec): string {
  const parts: string[] = [];
  for (const m of spec.includeFiles) parts.push(m);
  for (const m of spec.excludeFiles) parts.push(`-${m}`);
  for (const m of spec.includeDirs) parts.push(`${m}\\`);
  for (const m of spec.excludeDirs) parts.push(`-${m}\\`);
  return parts.join('; ');
}

// ------------------------------------------------------------------
// 名称过滤求值（mock 引擎与本地预演共用；引擎 filter.rs 全集语义子集）
// ------------------------------------------------------------------

/**
 * 文件是否通过名称过滤：
 * - includeFiles 非空：须命中其一（无命中 -> 排除）；
 * - excludeFiles 命中任一 -> 排除。
 * segments 为 rel 路径段；掩码匹配逐段（相对掩码 `p\f` 形态以段对齐）。
 */
export function fileAllowedBySpec(segments: string[], spec: NameFilterSpec | null): boolean {
  if (!spec) return true;
  const ci = spec.caseSensitive !== true;
  const last = segments[segments.length - 1] ?? '';
  if (spec.excludeFiles.length > 0) {
    for (const mask of spec.excludeFiles) {
      if (maskHitsPath(mask, segments, ci)) return false;
    }
  }
  if (spec.includeFiles.length > 0) {
    let hit = false;
    for (const mask of spec.includeFiles) {
      if (maskHitsPath(mask, segments, ci)) {
        hit = true;
        break;
      }
    }
    if (!hit) return false;
  }
  // 目录排除链（父目录命中排除 -> 不可见）
  if (spec.excludeDirs.length > 0 && segments.length > 1) {
    for (const seg of segments.slice(0, -1)) {
      for (const mask of spec.excludeDirs) {
        if (wildcardMatch(mask, seg, ci)) return false;
      }
    }
  }
  void last;
  return true;
}

/** 掩码 -> 路径段命中：`a/b` 形态按段对齐；单段掩码匹配末段 */
function maskHitsPath(mask: string, segments: string[], ci: boolean): boolean {
  const parts = mask.split('/').filter((p) => p.length > 0);
  if (parts.length > 1) {
    if (parts.length !== segments.length) return false;
    return parts.every((p, i) => wildcardMatch(p, segments[i], ci));
  }
  return wildcardMatch(mask, segments[segments.length - 1] ?? '', ci);
}

// ------------------------------------------------------------------
// 显示过滤器（FR-DIR-033；与引擎 display_visible 同口径）
// ------------------------------------------------------------------

export const SHOW_ALL_DISPLAY_FILTER: DisplayFilterSpec = {
  showSame: true,
  showContentDiff: true,
  showLeftNewer: true,
  showRightNewer: true,
  showLeftOnly: true,
  showRightOnly: true,
  showDirDiff: true,
};

/** 状态可见性（QuickDiff 按 mtime 分左右较新；与引擎同口径） */
export function displayVisible(row: DirRow, df: DisplayFilterSpec): boolean {
  switch (row.status) {
    case 'same':
    case 'dirSame':
      return df.showSame;
    case 'contentDiff':
      return df.showContentDiff;
    case 'dirDiff':
      return df.showDirDiff;
    case 'quickDiff': {
      const l = row.left?.mtimeMs ?? 0;
      const r = row.right?.mtimeMs ?? 0;
      return l >= r ? df.showLeftNewer : df.showRightNewer;
    }
    case 'leftOnly':
      return df.showLeftOnly;
    case 'rightOnly':
      return df.showRightOnly;
    default:
      return true;
  }
}

/**
 * 显示过滤 + 抑制重算（mock 引擎 / 本地预演共用）：
 * 返回新数组（不可变；不修改输入行）。
 */
export function applyDisplayToRows(
  rows: DirRow[],
  df: DisplayFilterSpec,
  suppress: boolean,
): DirRow[] {
  return rows.map((row) => {
    const hidden = !displayVisible(row, df);
    return { ...row, hidden: hidden && !suppress, suppressed: hidden && suppress };
  });
}

/** 常用显示过滤预设（工具栏下拉） */
export function displayFilterPreset(name: string): DisplayFilterSpec {
  switch (name) {
    case 'diffOnly':
      return { ...SHOW_ALL_DISPLAY_FILTER, showSame: false };
    case 'sameOnly':
      return {
        showSame: true,
        showContentDiff: false,
        showLeftNewer: false,
        showRightNewer: false,
        showLeftOnly: false,
        showRightOnly: false,
        showDirDiff: false,
      };
    case 'leftNewer':
      return {
        ...SHOW_ALL_DISPLAY_FILTER,
        showSame: false,
        showContentDiff: false,
        showRightNewer: false,
        showLeftOnly: false,
        showRightOnly: false,
        showDirDiff: false,
      };
    case 'rightNewer':
      return {
        ...SHOW_ALL_DISPLAY_FILTER,
        showSame: false,
        showContentDiff: false,
        showLeftNewer: false,
        showLeftOnly: false,
        showRightOnly: false,
        showDirDiff: false,
      };
    case 'leftOnly':
      return {
        ...SHOW_ALL_DISPLAY_FILTER,
        showSame: false,
        showContentDiff: false,
        showLeftNewer: false,
        showRightNewer: false,
        showRightOnly: false,
        showDirDiff: false,
      };
    case 'rightOnly':
      return {
        ...SHOW_ALL_DISPLAY_FILTER,
        showSame: false,
        showContentDiff: false,
        showLeftNewer: false,
        showRightNewer: false,
        showLeftOnly: false,
        showDirDiff: false,
      };
    default:
      return SHOW_ALL_DISPLAY_FILTER;
  }
}

/** 侧信息缺失工具（类型收窄） */
export function sideOf(row: DirRow, side: 'left' | 'right'): SideInfo | null {
  return side === 'left' ? row.left : row.right;
}

/** 目录行状态谓词（编辑菜单"选择较新/孤立"命令数据面） */
export function isOrphan(status: DirEntryStatus, side: 'left' | 'right'): boolean {
  return side === 'left' ? status === 'leftOnly' : status === 'rightOnly';
}

export function isNewer(row: DirRow, side: 'left' | 'right'): boolean {
  const l = row.left?.mtimeMs ?? 0;
  const r = row.right?.mtimeMs ?? 0;
  return row.status === 'quickDiff' && (side === 'left' ? l > r : r > l);
}
