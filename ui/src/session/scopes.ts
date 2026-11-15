import type { SettingsScope, SettingsTree } from './model';

/**
 * 设置作用域合并与写回（docs/03 §3.3 / §4.3，FR-SES-006）。
 *
 * 读取端 EffectiveSettings 按链逐层合并（浅两层结构：组 → 键 → 值）：
 *   global（settings.json）→ kind（session-defaults.json 的该类型默认）
 *   → parentKind（父会话"该类型所有文件"覆盖）→ parentPair（父会话"这对文件"覆盖）
 *   → session（会话实例 overrides）
 * 后层覆盖前层；origin 图记录每个组最终来自哪一层（UI 继承来源标注）。
 */

/** 作用域显示名（FR-SES-006 下拉，子会话两项仅在有父会话时可用） */
export const SCOPE_LABELS: Record<SettingsScope, string> = {
  session: '仅用于此会话',
  'parent-pair': '用于父会话中的这些文件',
  'parent-kind': '用于父会话中的所有文件',
  kind: '更新此类会话的默认值',
  global: '全局默认（所有会话类型）',
};

/** 拉平后的读取层顺序：低 → 高优先级 */
export type EffectiveLayer = 'global' | 'kind' | 'parentKind' | 'parentPair' | 'session';

export const LAYER_ORDER: readonly EffectiveLayer[] = [
  'global',
  'kind',
  'parentKind',
  'parentPair',
  'session',
];

export type OriginMap = { [group: string]: EffectiveLayer };

export interface EffectiveSettings {
  values: SettingsTree;
  /** 组级继承来源：标注"该项来自哪层"（docs/03 §4.3 显示层标注） */
  origin: OriginMap;
}

/** 深合并两棵设置树（覆盖层胜出；值为对象则递归，否则直接覆盖） */
export function mergeTrees(base: SettingsTree, override: SettingsTree): SettingsTree {
  const out: SettingsTree = {};
  for (const [g, group] of Object.entries(base)) out[g] = { ...group };
  for (const [g, group] of Object.entries(override)) {
    out[g] = { ...out[g], ...group };
  }
  return out;
}

/**
 * 逐层合并得到生效设置。layers 中 null/undefined 层跳过。
 * origin 取每个组"最后一次出现"的层（即最高优先级定义层）。
 */
export function resolveEffective(
  layers: Partial<Record<EffectiveLayer, SettingsTree | null | undefined>>,
): EffectiveSettings {
  const values: SettingsTree = {};
  const origin: OriginMap = {};
  for (const layer of LAYER_ORDER) {
    const tree = layers[layer];
    if (!tree) continue;
    for (const [g, group] of Object.entries(tree)) {
      if (group === undefined) continue;
      values[g] = { ...values[g], ...group };
      origin[g] = layer;
    }
  }
  return { values, origin };
}

/**
 * 把一组修改写回目标层的树（纯函数：返回新树，不改动入参）。
 * 写入按 组 → 键 粒度合并；键值为 undefined 表示"删除该项覆盖"（回落继承）。
 */
export function applyPatchToLayer(
  layer: SettingsTree | null | undefined,
  patch: SettingsTree,
): SettingsTree {
  const base: SettingsTree = layer ? { ...layer } : {};
  for (const [g, group] of Object.entries(patch)) {
    const merged = { ...base[g], ...group };
    for (const [k, v] of Object.entries(merged)) {
      if (v === undefined) delete merged[k];
    }
    base[g] = merged;
  }
  return base;
}

/** 单层读取：取某组设置（无则空对象） */
export function readGroup(
  tree: SettingsTree | null | undefined,
  group: string,
): { [key: string]: unknown } {
  return (tree && tree[group]) || {};
}

/** 作用域下拉的可用项：无父会话时隐藏子会话专属两项（FR-SES-006） */
export function availableScopes(hasParent: boolean): SettingsScope[] {
  const all: SettingsScope[] = ['session', 'parent-pair', 'parent-kind', 'kind', 'global'];
  return hasParent ? all : all.filter((s) => s !== 'parent-pair' && s !== 'parent-kind');
}

/** SettingsScope → EffectiveLayer 映射（写回目标层的读取链定位） */
export function scopeToLayer(scope: SettingsScope): EffectiveLayer | 'session-write' {
  switch (scope) {
    case 'session':
      return 'session-write';
    case 'parent-pair':
      return 'parentPair';
    case 'parent-kind':
      return 'parentKind';
    case 'kind':
      return 'kind';
    case 'global':
      return 'global';
  }
}
