import type { DirRow } from '../../protocol/types';
import { joinPath } from '../../lib/env';
import { buildTreeIndex, expandAll, orderedTreeRows } from '../folder-compare/dirTree';

/**
 * 子会话差异文件遍历（FR-DIR-071）。
 *
 * 遍历序 = 父会话整棵树的名称序（视为全部展开）：BC 的"下一个差异文件"走的是
 * 文件夹会话的完整树，不受当前展开状态限制。平铺模式下 id 是显示名、树形下
 * id 是相对路径，但 leftPath/rightPath 恒为真实相对路径，故定位当前对一律按
 * 路径而非 id（R3-03）。
 */

/** 两侧都在、且判定为有差异的文件才构成可打开的"差异文件对" */
const DIFF_STATUSES: ReadonlySet<string> = new Set(['quickDiff', 'contentDiff', 'similar']);

export type DiffFileRow = DirRow & { leftPath: string; rightPath: string };

function isDiffFileRow(r: DirRow): r is DiffFileRow {
  return (
    r.kind !== 'dir' &&
    DIFF_STATUSES.has(r.status) &&
    typeof r.leftPath === 'string' &&
    typeof r.rightPath === 'string'
  );
}

/**
 * 差异文件对（整棵树名称序）。
 *
 * hidden 是显示过滤器的呈现标记，引擎明示"不影响比较结果"（ai-protocol
 * types.rs DirRow），故遍历前一律抹掉：父视图切到"仅差异"时 dirSame 目录被
 * 隐藏，orderedTreeRows 会连同整棵子树跳过，35 个差异文件只剩顶层 1 个
 * （真实浏览器实测）。suppressed 只是同一批隐藏行的青绿呈现，不影响判定。
 */
export function orderedDiffFileRows(rows: readonly DirRow[]): DiffFileRow[] {
  const all = rows.map((r) => (r.hidden ? { ...r, hidden: false } : r));
  return orderedTreeRows(buildTreeIndex(all), expandAll(all)).filter(isDiffFileRow);
}

/** 当前子会话正在比较的对在遍历序中的位置；-1 = 不在差异列表里（如已保存成相同） */
export function currentDiffIndex(
  rows: readonly DirRow[],
  current: { leftPath: string; rightPath: string },
  roots: { left: string; right: string },
): number {
  return rows.findIndex(
    (r) =>
      typeof r.leftPath === 'string' &&
      typeof r.rightPath === 'string' &&
      joinPath(roots.left, r.leftPath) === current.leftPath &&
      joinPath(roots.right, r.rightPath) === current.rightPath,
  );
}

/** 回绕取下一个/上一个；当前对不在列表时从首/尾进入 */
export function pickDiffTarget(
  ordered: readonly DiffFileRow[],
  index: number,
  dir: 'next' | 'prev',
): DiffFileRow | null {
  if (ordered.length === 0) return null;
  if (index < 0) return dir === 'next' ? ordered[0] : ordered[ordered.length - 1];
  const step = dir === 'next' ? 1 : -1;
  return ordered[(index + step + ordered.length) % ordered.length];
}
