import type { SettingsScope } from '../../session/model';

/**
 * 设置对话框 schema（FR-SES-002：分页选项卡 + 作用域下拉；M34 表单 schema 渲染器）。
 *
 * 三页起步（docs/04 §P4）：比较 / 同步 / 通用。字段声明驱动表单渲染
 * （SchemaForm），值按"组.键"落入 SettingsTree；同步页字段为 P8 数据位
 * （先可配置、引擎消费属 P8）。
 */

export type FieldType = 'number' | 'boolean' | 'select' | 'text';

export interface FieldDef {
  /** SettingsTree 内的键（组内路径，如 'mtimeToleranceSec'） */
  key: string;
  label: string;
  type: FieldType;
  /** select 的选项 */
  options?: Array<{ value: string; label: string }>;
  min?: number;
  max?: number;
  step?: number;
  /** 帮助文本 */
  help?: string;
  /** 未交付字段（置灰 + 阶段说明，如同步页 P8） */
  phaseNote?: string;
}

export interface SettingsPage {
  id: string;
  title: string;
  icon: string;
  /** 该页适用的作用域集（程序选项页不含会话作用域） */
  scope: 'session-scoped' | 'app';
  fields: FieldDef[];
}

export const COMPARE_PAGE: SettingsPage = {
  id: 'compare',
  title: '比较',
  icon: '⚖️',
  scope: 'session-scoped',
  fields: [
    {
      key: 'mtimeToleranceSec',
      label: '时间戳容差（秒）',
      type: 'number',
      min: 0,
      max: 3600,
      step: 1,
      help: '两侧修改时间相差在该容差内视为相同（引擎 CompareCriteria.mtimeToleranceSec）',
    },
    {
      key: 'contentCompare',
      label: '内容比较',
      type: 'select',
      options: [
        { value: 'never', label: '不比较内容（仅时间戳/大小）' },
        { value: 'auto', label: '自动（时间戳不同才读内容）' },
        { value: 'always', label: '总是比较内容' },
      ],
      help: '快速相同跳过/覆盖策略（ContentCompareMode）',
    },
    {
      key: 'ignoreDst',
      label: '忽略夏令时差异',
      type: 'boolean',
      phaseNote: 'DST 判据属 P2 快速测试全集',
    },
    {
      key: 'ignoreTimestamp',
      label: '忽略时间戳差异',
      type: 'boolean',
      phaseNote: '判据扩展属 P2 快速测试全集',
    },
  ],
};

export const SYNC_PAGE: SettingsPage = {
  id: 'sync',
  title: '同步',
  icon: '🔄',
  scope: 'session-scoped',
  fields: [
    {
      key: 'method',
      label: '同步方法',
      type: 'select',
      options: [
        { value: 'update', label: '更新（较新者胜）' },
        { value: 'mirrorLeftToRight', label: '镜像（左 → 右）' },
        { value: 'mirrorRightToLeft', label: '镜像（右 → 左）' },
      ],
      phaseNote: '同步状态机 UI 属 P8（SYNC-002），当前仅保存配置',
    },
    {
      key: 'syncDeletions',
      label: '同步删除操作',
      type: 'boolean',
      phaseNote: '属 P8',
    },
    {
      key: 'includeEmptyDirs',
      label: '包括空文件夹',
      type: 'boolean',
      phaseNote: '属 P8',
    },
  ],
};

export const GENERAL_PAGE: SettingsPage = {
  id: 'general',
  title: '通用',
  icon: '⚙️',
  scope: 'app',
  fields: [
    {
      key: 'showHomeOnStart',
      label: '启动时显示主页视图',
      type: 'boolean',
      help: 'SET-005：默认显示主页；关闭后恢复上次工作区（P13）',
    },
    {
      key: 'closeWindowOnLastTab',
      label: '关闭最后一个标签页时关闭窗口',
      type: 'boolean',
      help: 'FR-WIN-001 标签页选项（当前默认回到主页标签）',
    },
    {
      key: 'confirmBeforeSave',
      label: '关闭会话前确认保存',
      type: 'boolean',
      help: 'FR-SES-001：修改命名会话后退出时提示（当前始终开启）',
    },
    {
      key: 'showSessionPanel',
      label: '显示会话管理面板',
      type: 'boolean',
      help: 'FR-HOME-002：主页左侧会话树常驻（F9 切换）',
    },
  ],
};

export const SETTINGS_PAGES: readonly SettingsPage[] = [COMPARE_PAGE, SYNC_PAGE, GENERAL_PAGE];

export function pageById(id: string | undefined): SettingsPage {
  return SETTINGS_PAGES.find((p) => p.id === id) ?? COMPARE_PAGE;
}

/** 会话作用域下拉项（FR-SES-006：scope 由可用层过滤） */
export interface ScopeOption {
  value: SettingsScope;
  label: string;
}

/** 字段默认值（schema 内聚，避免散落的 magic default） */
export function fieldDefault(field: FieldDef): unknown {
  switch (field.type) {
    case 'number':
      return field.key === 'mtimeToleranceSec' ? 2 : 0;
    case 'boolean':
      return false;
    case 'select':
      return field.options?.[0]?.value ?? '';
    case 'text':
      return '';
  }
}

/** 读取某组某键的值（层缺失回落默认值） */
export function readFieldValue(
  layer: { [group: string]: { [key: string]: unknown } } | null | undefined,
  group: string,
  field: FieldDef
): unknown {
  const v = layer?.[group]?.[field.key];
  return v === undefined ? fieldDefault(field) : v;
}
