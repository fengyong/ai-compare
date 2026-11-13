/**
 * 批量重命名模板引擎（FR-DIR-064 多文件重命名的纯函数数据面，P6b 批）：
 * - DOS 掩码模式：`*` 保留原名基底（去扩展名）、`?` 保留原基底在当前输出
 *   位置的字符（与 BC 自动基掩码 `abc?.bak` 逐位对齐语义一致）、
 *   `#` 串 = 计数器占位（按位数零填充，首个 `#` 串生效、后续按字面量），
 *   如 `IMG####.jpg` / `abc?.bak`（`?` 保留原字符生成唯一名）；
 * - 正则模板模式：旧掩码（JS 正则）匹配原名，替换模板支持 `$1` 组引用；
 *   未命中行保留原名（非错误）；非法正则/越界组引用 => 该行错误；
 * - 冲突检测（预览拦截）：批次内重名（大小写不敏感，Windows/macOS 惯例）、
 *   与同父未参与重命名的现有行撞名、计算结果含非法字符、空名。
 * 执行链：plan.renames -> dirOps rename ops（子项先于父项，引擎按旧路径
 * 定位源、重写子树前缀，父项后执行避免子路径失配）。
 */

import type { DirRow } from '../../protocol/types';
import { parentOf } from './dirTree';

export type RenameMode = 'dos' | 'regex';

export interface DosRenameOptions {
  mask: string;
  /** 计数器起始值（默认 1） */
  counterStart: number;
}

export interface RegexRenameOptions {
  /** 旧掩码（JS 正则源文本） */
  pattern: string;
  /** 替换模板（支持 $1..$9 / $& 组引用） */
  template: string;
}

/** 文件名非法字符（跨平台保守集：POSIX 分隔符 + Windows 保留集 + 控制字符） */
// eslint-disable-next-line no-control-regex -- 非法字符集刻意覆盖控制字符（文件名净化）
const ILLEGAL_CHARS = /[/\\:*?"<>|\u0000-\u001f]/;

export function firstIllegalChar(name: string): string | null {
  const m = name.match(ILLEGAL_CHARS);
  return m ? m[0] : null;
}

/** 原名拆分：基底 + 扩展名（点文件 `.gitignore` 视为无扩展名） */
export function splitBaseExt(name: string): { base: string; ext: string } {
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return { base: name, ext: '' };
  return { base: name.slice(0, dot), ext: name.slice(dot) };
}

/**
 * DOS 掩码应用（单行）。
 * 语义（BC 多文件重命名 DOS 兼容子集，与测试锚定）：
 * - `*`  = 原名基底（不含扩展名）；
 * - `?`  = 原基底在当前输出位置的字符（以已产出长度为索引；越界产出空）——
 *   自动基掩码（所选名称重叠前缀 + `?`）逐位保留原字符；
 * - `#`  = 计数器（首个 `#` 串，宽度 = 串长，零填充；后续 `#` 串按字面量）；
 * - 其余字符按字面量（扩展名需在掩码中显式书写，如 `*.bak`）。
 */
export function applyDosMask(originalName: string, options: DosRenameOptions, counter: number): string {
  const { base } = splitBaseExt(originalName);
  const mask = options.mask;
  let out = '';
  let counterRunUsed = false;
  for (let i = 0; i < mask.length; i++) {
    const c = mask[i];
    if (c === '*') {
      out += base;
      continue;
    }
    if (c === '?') {
      // 位置对齐：`?` 在输出中的当前位置即原基底字符位（越界产出空）
      out += out.length < base.length ? base[out.length] : '';
      continue;
    }
    if (c === '#') {
      let j = i;
      while (j < mask.length && mask[j] === '#') j++;
      if (!counterRunUsed) {
        counterRunUsed = true;
        out += String(counter).padStart(j - i, '0');
      } else {
        out += '#'.repeat(j - i); // 第二个 `#` 串按字面量（BC 取首个为计数器）
      }
      i = j - 1;
      continue;
    }
    out += c;
  }
  return out;
}

/** 正则模式编译（视图输入时即时校验；错误信息面向预览行） */
export function compileRegex(options: RegexRenameOptions): { re: RegExp; groupCount: number } | { error: string } {
  if (!options.pattern) return { error: '旧掩码为空' };
  let re: RegExp;
  try {
    re = new RegExp(options.pattern);
  } catch (err) {
    return { error: `非法正则：${err instanceof Error ? err.message : String(err)}` };
  }
  // 组计数技巧：`pattern|` 空串匹配，捕获组数 = match 长度 - 1
  let groupCount = 0;
  try {
    groupCount = new RegExp(`${options.pattern}|`).exec('')!.length - 1;
  } catch {
    groupCount = 0;
  }
  // 模板组引用越界校验（$1..$99 / $&）
  for (const m of options.template.matchAll(/\$(\d{1,2})/g)) {
    const n = Number(m[1]);
    if (n > groupCount) return { error: `模板引用了不存在的组 $${n}（掩码仅有 ${groupCount} 个捕获组）` };
  }
  return { re, groupCount };
}

/** 正则模板应用（单行）：未命中返回 null（保留原名，非错误）；编译错误抛回调用方 */
export function applyRegexTemplate(originalName: string, options: RegexRenameOptions, compiled: { re: RegExp }): string | null {
  if (!compiled.re.test(originalName)) return null;
  return originalName.replace(compiled.re, options.template);
}

// ------------------------------------------------------------------
// 批量计划（预览表 + 冲突拦截）
// ------------------------------------------------------------------

export type RenameItemState = 'rename' | 'unchanged' | 'invalid';
export type RenameConflict = 'duplicate' | 'exists';

export interface RenamePlanItem {
  row: DirRow;
  oldName: string;
  /** 计算结果；invalid 时为 null */
  newName: string | null;
  parent: string;
  state: RenameItemState;
  /** invalid/错误原因 */
  reason: string | null;
  conflict: RenameConflict | null;
  /** 冲突详情（duplicate/exists 的对端说明） */
  conflictReason: string | null;
}

export interface RenamePlan {
  items: RenamePlanItem[];
  /** 可执行重命名（state=rename 且无冲突；按「子先父后」排序） */
  renames: RenamePlanItem[];
  /** 冲突/非法行数（>0 时执行拦截） */
  conflicts: number;
  unchanged: number;
}

export interface RenamePlanInput {
  /** 参与重命名的行（当前选择的可见文件/目录） */
  rows: readonly DirRow[];
  /** 同父层现有行（撞名检测的背景集；通常传会话全部行） */
  allRows: readonly DirRow[];
  mode: RenameMode;
  dos: DosRenameOptions;
  regex: RegexRenameOptions;
}

/**
 * 批量重命名计划：逐行计算新名 -> 状态/冲突判定。
 * 计数器按输入行序递增（每行消耗一个计数值，保持预览稳定）。
 */
export function planRenames(input: RenamePlanInput): RenamePlan {
  const compiled = input.mode === 'regex' ? compileRegex(input.regex) : null;
  const selectedIds = new Set(input.rows.map((r) => r.id));

  const items: RenamePlanItem[] = [];
  let counter = Math.max(1, Math.floor(input.dos.counterStart || 1));
  for (const row of input.rows) {
    const parent = parentOf(row.id);
    let newName: string | null = null;
    let reason: string | null = null;
    if (input.mode === 'dos') {
      newName = applyDosMask(row.name, input.dos, counter);
    } else {
      if (compiled && 'error' in compiled) {
        reason = compiled.error;
      } else if (compiled) {
        newName = applyRegexTemplate(row.name, input.regex, compiled);
        if (newName === null) {
          // 未命中：保留原名（BC 惯例：旧掩码未匹配的行不动）
          newName = row.name;
        }
      }
    }
    counter++;

    if (reason === null) {
      if (newName === null || newName.length === 0) {
        reason = '新名称为空';
        newName = null;
      } else if (newName === '.' || newName === '..') {
        reason = '新名称非法（保留名）';
        newName = null;
      } else {
        const bad = firstIllegalChar(newName);
        if (bad) {
          reason = `含非法字符「${bad === '\t' ? '\\t' : bad}」`;
          newName = null;
        }
      }
    }

    const state: RenameItemState = reason !== null ? 'invalid' : newName === row.name ? 'unchanged' : 'rename';
    items.push({ row, oldName: row.name, newName, parent, state, reason, conflict: null, conflictReason: null });
  }

  // ---- 冲突判定（仅 state=rename 参与） ----
  // 1) 批次内重名（同父 + 大小写不敏感同名）
  const byKey = new Map<string, RenamePlanItem[]>();
  for (const item of items) {
    if (item.state !== 'rename' || item.newName === null) continue;
    const key = `${item.parent}\u0000${item.newName.toLowerCase()}`;
    const list = byKey.get(key);
    if (list) list.push(item);
    else byKey.set(key, [item]);
  }
  for (const list of byKey.values()) {
    if (list.length < 2) continue;
    for (const item of list) {
      item.conflict = 'duplicate';
      item.conflictReason = `批次内 ${list.length} 项重命名为「${item.newName}」`;
    }
  }
  // 2) 与未参与重命名的现有行撞名（同父层；大小写不敏感）
  const existingByParent = new Map<string, Set<string>>();
  for (const row of input.allRows) {
    if (selectedIds.has(row.id)) continue; // 参与重命名的行不构成背景撞名
    const parent = parentOf(row.id);
    let set = existingByParent.get(parent);
    if (!set) {
      set = new Set();
      existingByParent.set(parent, set);
    }
    set.add(row.name.toLowerCase());
  }
  for (const item of items) {
    if (item.state !== 'rename' || item.newName === null || item.conflict !== null) continue;
    const siblings = existingByParent.get(item.parent);
    if (siblings && siblings.has(item.newName.toLowerCase())) {
      item.conflict = 'exists';
      item.conflictReason = `目标位置已存在同名项「${item.newName}」`;
    }
  }

  const renames = items
    .filter((it) => it.state === 'rename' && it.conflict === null)
    // 子先父后：引擎按旧路径定位源并重写子树前缀，父项后执行避免子路径失配
    .sort((a, b) => depthOf(b.row.id) - depthOf(a.row.id) || (a.row.id < b.row.id ? -1 : 1));
  const conflicts = items.filter((it) => it.state === 'invalid' || it.conflict !== null).length;
  const unchanged = items.filter((it) => it.state === 'unchanged').length;
  return { items, renames, conflicts, unchanged };
}

function depthOf(id: string): number {
  let d = 0;
  for (const ch of id) if (ch === '/') d++;
  return d;
}

/** 重命名清单 -> dir/execute rename ops（side 取存在侧：左优先，与单项重命名同口径） */
export function renamesToOps(
  renames: readonly { row: DirRow; newName: string }[]
): { kind: 'rename'; path: string; side: 'left' | 'right'; newName: string }[] {
  return renames.map((it) => ({
    kind: 'rename' as const,
    path: it.row.id,
    side: it.row.left ? ('left' as const) : ('right' as const),
    newName: it.newName,
  }));
}
