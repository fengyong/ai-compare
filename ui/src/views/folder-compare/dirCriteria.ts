/**
 * 目录会话判据的来源（R3-16，FR-DIR-011/012）：设置页「比较」组 → 引擎
 * CompareCriteria。此前视图硬编码 DEFAULT_DIR_CRITERIA，设置页的内容比较
 * 选项写了也不生效；这里把生效设置（global → kind → session）接进 dir/open。
 */

import { sessionService } from '../../session/service';
import { readGroup } from '../../session/scopes';
import {
  defaultCompareRulesState,
  toRulesSpec,
  type CompareRulesState,
} from '../../shell/settings/rulesModel';
import {
  DEFAULT_DIR_CRITERIA,
  type CompareCriteria,
  type ContentCompareMethod,
  type ContentCompareMode,
  type RulesSpec,
} from '../../protocol/types';

/** BC 会话设置「比较内容」的三种方法（FR-DIR-011） */
export const CONTENT_METHODS: readonly ContentCompareMethod[] = ['crc', 'binary', 'rulesBased'];

const CONTENT_MODES: readonly ContentCompareMode[] = ['never', 'auto', 'always'];

export function isContentMethod(v: unknown): v is ContentCompareMethod {
  return typeof v === 'string' && (CONTENT_METHODS as readonly string[]).includes(v);
}

export function isContentMode(v: unknown): v is ContentCompareMode {
  return typeof v === 'string' && (CONTENT_MODES as readonly string[]).includes(v);
}

/**
 * compare 组 → 引擎判据。非法值（手改 settings.json / 旧版本残留）回落引擎
 * 默认，而不是把 undefined 传进引擎导致整份判据被 serde 拒绝。
 */
export function criteriaFromCompareGroup(group: { [key: string]: unknown }): CompareCriteria {
  const tol = group['mtimeToleranceSec'];
  const mode = group['contentCompare'];
  const method = group['contentMethod'];
  return {
    ...DEFAULT_DIR_CRITERIA,
    mtimeToleranceSec:
      typeof tol === 'number' && Number.isFinite(tol) && tol >= 0
        ? Math.floor(tol)
        : DEFAULT_DIR_CRITERIA.mtimeToleranceSec,
    contentCompare: isContentMode(mode) ? mode : DEFAULT_DIR_CRITERIA.contentCompare,
    contentMethod: isContentMethod(method) ? method : DEFAULT_DIR_CRITERIA.contentMethod,
  };
}

/**
 * compareRules 组 → 引擎规则载荷。空组 = 未配置 → undefined（引擎默认）。
 * 目录内文件格式各异，语法元素集无法按会话统一绑定 → 空集，只生效
 * 空白/大小写/行尾/编码开关。
 */
export function rulesFromCompareRulesGroup(group: {
  [key: string]: unknown;
}): RulesSpec | undefined {
  if (Object.keys(group).length === 0) return undefined;
  return toRulesSpec(
    { ...defaultCompareRulesState(), ...(group as Partial<CompareRulesState>) },
    null,
  );
}

/**
 * 读生效设置得到目录会话判据。sessionNodeId 为命名会话 id（tab.sessionLink），
 * 未命名会话只取 global + kind 两层。设置层未初始化/损坏时回落默认判据。
 */
export function effectiveDirCriteria(sessionNodeId?: string | null): CompareCriteria {
  try {
    return criteriaFromCompareGroup(readGroup(folderEffectiveValues(sessionNodeId), 'compare'));
  } catch {
    return { ...DEFAULT_DIR_CRITERIA };
  }
}

/** 目录会话的规则载荷（contentMethod=rulesBased 时决定哪些差异不重要） */
export function effectiveDirRules(sessionNodeId?: string | null): RulesSpec | undefined {
  try {
    return rulesFromCompareRulesGroup(
      readGroup(folderEffectiveValues(sessionNodeId), 'compareRules'),
    );
  } catch {
    return undefined;
  }
}

function folderEffectiveValues(sessionNodeId?: string | null) {
  const overrides = sessionNodeId ? sessionService.getSession(sessionNodeId)?.overrides : undefined;
  return sessionService.resolveSessionEffective('folder-compare', overrides).values;
}
