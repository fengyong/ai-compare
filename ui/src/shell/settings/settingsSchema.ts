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
  /** 字段缺省值（缺省按 type 推导；布尔开关需非 false 缺省时显式声明） */
  default?: unknown;
}

export interface SettingsPage {
  id: string;
  title: string;
  icon: string;
  /** 该页适用的作用域集（程序选项页不含会话作用域） */
  scope: 'session-scoped' | 'app';
  fields: FieldDef[];
  /**
   * P10：结构化页（SchemaForm 之外的专用编辑器组件）。
   * - 'fileFormats'：格式列表/掩码/语法元素/导入导出（FileFormatsPage）；
   * - 'compareRules'：重要性开关 + 元素勾选 + 格式绑定（CompareRulesPage）。
   * fields 仍声明该页的标量字段（生效预览与默认值口径），由专用组件渲染。
   */
  custom?: 'fileFormats' | 'compareRules';
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

/**
 * P10 文件格式页（FR-FMT-001）：格式列表/掩码/语法元素/导入导出。
 * 格式为程序级资源（BC 语义）→ 作用域 app（settings.json 全局层）。
 * fields 声明两个持久化键的口径（值由 FileFormatsPage 结构化写入）。
 */
export const FILE_FORMATS_PAGE: SettingsPage = {
  id: 'fileFormats',
  title: '文件格式',
  icon: '🧩',
  scope: 'app',
  custom: 'fileFormats',
  fields: [
    {
      key: 'custom',
      label: '自定义格式',
      type: 'text',
      help: '自定义格式定义（FileFormatsPage 结构化编辑；此处仅生效预览）',
    },
    {
      key: 'builtinOverrides',
      label: '内置格式覆盖',
      type: 'text',
      help: '内置格式的启用/掩码覆盖（FileFormatsPage 结构化编辑）',
    },
  ],
};

/**
 * P10 比较规则页（FR-TXT-010）：重要性开关 + 元素勾选 + 格式绑定。
 * 会话级资源 → session-scoped（四层作用域；缺省写 session-defaults.json
 * 的类型默认层）。fields 与 CompareRulesState 的开关一一对应
 * （eol/encoding 缺省 true = 引擎 ImportanceOptions::default）。
 */
export const COMPARE_RULES_PAGE: SettingsPage = {
  id: 'compareRules',
  title: '比较规则',
  icon: '📋',
  scope: 'session-scoped',
  custom: 'compareRules',
  fields: [
    { key: 'leadingWsUnimportant', label: '行首空白不重要', type: 'boolean', help: 'FR-TXT-010 空白三类' },
    { key: 'embeddedWsUnimportant', label: '嵌入空白不重要', type: 'boolean', help: '连续空白折叠为一个空格' },
    { key: 'trailingWsUnimportant', label: '行尾空白不重要', type: 'boolean', help: 'FR-TXT-010 空白三类' },
    { key: 'caseUnimportant', label: '字符大小写不重要', type: 'boolean', help: '"其他所有内容"区分大小写开关的反面' },
    { key: 'eolStyleUnimportant', label: '行尾风格不重要', type: 'boolean', default: true, help: 'CRLF/LF/CR 差异不重要（引擎缺省 true）' },
    { key: 'encodingUnimportant', label: '纯编码差异不重要', type: 'boolean', default: true, help: '引擎缺省 true' },
    { key: 'formatBindings', label: '格式绑定（不重要元素）', type: 'text', help: 'formatId -> 不重要语法元素名集（CompareRulesPage 结构化编辑）' },
  ],
};

export const SETTINGS_PAGES: readonly SettingsPage[] = [
  COMPARE_PAGE,
  SYNC_PAGE,
  GENERAL_PAGE,
  FILE_FORMATS_PAGE,
  COMPARE_RULES_PAGE,
];

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
  if (field.default !== undefined) return field.default;
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
