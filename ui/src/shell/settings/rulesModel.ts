import { effectiveFormats } from './formatsModel';
import type { FileFormatsState } from './formatsModel';

/**
 * 比较规则页的纯逻辑层（P10 / FR-TXT-010 / FR-SES-006 设置侧）。
 *
 * 职责：
 * - 持久化状态形态（比较规则组 `compareRules`，作用域四层可写：
 *   全局 / 类型默认（session-defaults.json）/ 会话 / 父会话层）：
 *   六个重要性开关 + formatBindings（格式绑定：formatId -> 不重要元素名集）；
 * - 开关 -> 引擎 RulesSpec / ImportanceOptionsSpec 的映射（字段缺省与引擎
 *   ImportanceOptions::default 一致：行尾风格与纯编码差异不重要）；
 * - 元素勾选（抽象层）：按所选格式列出语法元素名（引用格式层定义，SRS 2.4
 *   "格式定义匹配规则、会话勾选哪些不重要"的两层协作）；
 * - 层合并（读取端生效预览）：组内键浅合并、数组/对象整体覆盖（与
 *   scopes.mergeTrees 语义一致）。
 *
 * 全部纯函数（无 React / 无 IO），供组件与单测共用。
 */

/** 设置树中 compareRules 组的持久化形态 */
export interface CompareRulesState {
  /** 行首空白不重要 */
  leadingWsUnimportant: boolean;
  /** 嵌入空白不重要（连续空白折叠为一个空格） */
  embeddedWsUnimportant: boolean;
  /** 行尾空白不重要 */
  trailingWsUnimportant: boolean;
  /** 字符大小写不重要（"其他所有内容"区分大小写开关的反面） */
  caseUnimportant: boolean;
  /** 行尾风格（CRLF/LF/CR）不重要——引擎缺省 true */
  eolStyleUnimportant: boolean;
  /** 纯编码差异不重要——引擎缺省 true */
  encodingUnimportant: boolean;
  /** 格式绑定：formatId -> 不重要语法元素名集（引用格式层元素名） */
  formatBindings: { [formatId: string]: string[] };
}

/** 引擎缺省（ai-core ImportanceOptions::default；docs/03 §6.1） */
export const DEFAULT_COMPARE_RULES: CompareRulesState = {
  leadingWsUnimportant: false,
  embeddedWsUnimportant: false,
  trailingWsUnimportant: false,
  caseUnimportant: false,
  eolStyleUnimportant: true,
  encodingUnimportant: true,
  formatBindings: {},
};

export function defaultCompareRulesState(): CompareRulesState {
  return { ...DEFAULT_COMPARE_RULES, formatBindings: {} };
}

/**
 * 开关 -> 引擎侧 RulesSpec 载荷（text/open params.rules / ImportanceOptionsSpec）。
 * 全字段具值（序列化时 None 语义由引擎按缺省补齐，值一致）。
 */
export function toImportanceOptionsSpec(state: CompareRulesState): {
  leadingWsUnimportant: boolean;
  embeddedWsUnimportant: boolean;
  trailingWsUnimportant: boolean;
  caseUnimportant: boolean;
  eolStyleUnimportant: boolean;
  encodingUnimportant: boolean;
} {
  return {
    leadingWsUnimportant: state.leadingWsUnimportant,
    embeddedWsUnimportant: state.embeddedWsUnimportant,
    trailingWsUnimportant: state.trailingWsUnimportant,
    caseUnimportant: state.caseUnimportant,
    eolStyleUnimportant: state.eolStyleUnimportant,
    encodingUnimportant: state.encodingUnimportant,
  };
}

/**
 * 生效规则载荷（引擎 RulesSpec 全形）：开关 + 指定格式的元素集。
 * formatId 未绑定（或无格式）时 unimportantElements 为空集 = 不启用语法折叠。
 */
export function toRulesSpec(
  state: CompareRulesState,
  formatId: string | null
): {
  leadingWsUnimportant: boolean;
  embeddedWsUnimportant: boolean;
  trailingWsUnimportant: boolean;
  caseUnimportant: boolean;
  eolStyleUnimportant: boolean;
  encodingUnimportant: boolean;
  unimportantElements: string[];
} {
  return {
    ...toImportanceOptionsSpec(state),
    unimportantElements: formatId ? state.formatBindings[formatId] ?? [] : [],
  };
}

// ---------------------------------------------------------------------------
// 元素勾选（抽象层，FR-TXT-010 重要性选项卡）
// ---------------------------------------------------------------------------

/** 某格式的语法元素名列表（引用格式层定义；内置目录 + 自定义格式） */
export function elementsOfFormat(
  formatsState: FileFormatsState,
  formatId: string | null
): string[] {
  if (!formatId) return [];
  const fmt = effectiveFormats(formatsState).find((f) => f.id === formatId);
  return fmt ? fmt.grammar.map((e) => e.name) : [];
}

/** 指定格式的生效"不重要元素"集（绑定优先；未绑定 = 空集） */
export function effectiveUnimportantElements(state: CompareRulesState, formatId: string | null): string[] {
  if (!formatId) return [];
  return state.formatBindings[formatId] ?? [];
}

/** 勾选/取消一个元素（纯函数；返回新 bindings 与新 state） */
export function toggleElement(
  state: CompareRulesState,
  formatId: string,
  elementName: string
): CompareRulesState {
  const current = state.formatBindings[formatId] ?? [];
  const next = current.includes(elementName)
    ? current.filter((e) => e !== elementName)
    : [...current, elementName].sort();
  return {
    ...state,
    formatBindings: { ...state.formatBindings, [formatId]: next },
  };
}

/** 设置整个绑定（从其他格式复制勾选等场景） */
export function setBinding(
  state: CompareRulesState,
  formatId: string,
  elements: string[]
): CompareRulesState {
  return {
    ...state,
    formatBindings: { ...state.formatBindings, [formatId]: [...elements].sort() },
  };
}

// ---------------------------------------------------------------------------
// 层合并（读取端生效预览；与 scopes.resolveEffective 的组内合并语义一致）
// ---------------------------------------------------------------------------

/**
 * 逐层合并 compareRules 组（低 -> 高优先级；键浅合并，
 * formatBindings 对象整体覆盖——与 mergeTrees 的"值为对象则覆盖"一致）。
 * 层为 null/undefined 跳过。
 */
export function mergeRuleLayers(
  layers: Array<Partial<CompareRulesState> | null | undefined>
): CompareRulesState {
  const out = defaultCompareRulesState();
  for (const layer of layers) {
    if (!layer) continue;
    for (const [k, v] of Object.entries(layer)) {
      if (v === undefined) continue;
      (out as unknown as Record<string, unknown>)[k] = v;
    }
  }
  return out;
}

/**
 * 合并后取生效元素集：优先取最高层的 formatBindings[formatId]；
 * 高层未定义该格式的绑定时回落低层（per-format 回落，非整体覆盖）。
 */
export function effectiveElementsAcrossLayers(
  layers: Array<Partial<CompareRulesState> | null | undefined>,
  formatId: string | null
): string[] {
  for (let i = layers.length - 1; i >= 0; i--) {
    const layer = layers[i];
    if (!layer?.formatBindings) continue;
    const binding = layer.formatBindings[formatId ?? ''];
    if (binding !== undefined) return binding;
  }
  return [];
}
