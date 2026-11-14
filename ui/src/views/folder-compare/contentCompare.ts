/**
 * 手动内容比较（R3-16，FR-DIR-012「操作 > 比较内容」）的数据面：
 * 目标行选择、能力位判定、五态结果摘要。视图层只负责渲染与调用 RPC，
 * 判定口径全部在这里（可在 node 环境单测，不依赖 jsdom）。
 */

import type {
  ContentCompareMethod,
  ContentCompareResult,
  DirCompareContentParams,
  DirContentCompareRow,
  DirRow,
  EngineCapabilities,
} from '../../protocol/types';
import { F } from './localStrings';

/** 旧引擎无 dir/compareContent 入口：调用方必须提示不支持，不得当成"已比较" */
export function contentCompareSupported(caps: EngineCapabilities | null | undefined): boolean {
  return caps?.dirCompareContent === true;
}

/** 只有两侧都存在的文件行能比较内容（目录行/孤立项跳过，BC 语义） */
export function isContentComparable(row: DirRow): boolean {
  return row.kind === 'file' && row.left !== null && row.right !== null;
}

/**
 * 选中行 → 待比较 id。未选中任何可比较行时返回空数组 = 交引擎比较全部
 * 已配对文件行（dir/compareContent 的 ids 空语义）。
 */
export function compareTargetIds(rows: readonly DirRow[], selected: ReadonlySet<string>): string[] {
  return rows.filter((r) => selected.has(r.id) && isContentComparable(r)).map((r) => r.id);
}

export function buildCompareContentParams(
  sessionId: string,
  ids: readonly string[],
  method?: ContentCompareMethod | null,
): DirCompareContentParams {
  return { sessionId, ids: [...ids], method: method ?? null };
}

export const RESULT_LABELS: Record<ContentCompareResult, string> = {
  binarySame: F.contentCompareBinarySame,
  binaryDiff: F.contentCompareBinaryDiff,
  rulesSame: F.contentCompareRulesSame,
  unimportantDiff: F.contentCompareUnimportantDiff,
  importantDiff: F.contentCompareImportantDiff,
};

export interface ContentCompareSummary {
  total: number;
  counts: Record<ContentCompareResult, number>;
  /** 一行摘要（结果对话框标题下方） */
  message: string;
}

const EMPTY_COUNTS: Record<ContentCompareResult, number> = {
  binarySame: 0,
  binaryDiff: 0,
  rulesSame: 0,
  unimportantDiff: 0,
  importantDiff: 0,
};

/** 五态计数 + 摘要文案（FR-DIR-012 结果枚举；CRC/二进制只产出前两态） */
export function summarizeContentCompare(
  results: readonly DirContentCompareRow[],
): ContentCompareSummary {
  const counts = { ...EMPTY_COUNTS };
  for (const r of results) counts[r.result] += 1;
  const parts = (Object.keys(counts) as ContentCompareResult[])
    .filter((k) => counts[k] > 0)
    .map((k) => `${RESULT_LABELS[k]} ${counts[k]}`);
  return {
    total: results.length,
    counts,
    message: parts.length > 0 ? parts.join('，') : F.contentCompareEmpty,
  };
}
