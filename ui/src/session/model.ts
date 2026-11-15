/**
 * 会话层领域模型（docs/03 §3.3 会话层 / §4.3 会话树与作用域 / FR-SES-001~006）。
 *
 * 数据文件（SET-001，对齐 BC 的 BCSessions.xml/BCState.xml 模型）：
 * - sessions.json          会话树（类型化会话节点 + 嵌套文件夹）+ 会话层 overrides
 * - session-defaults.json  每类会话的默认设置（FR-SES-003，主页树"新建"分支编辑）
 * - settings.json          全局程序选项
 * - state.json             MRU（最近会话/最近路径，按类型分列）+ UI 状态
 *
 * 作用域合并（FR-SES-006）：读取端逐层合并（global → kind → parent → session），
 * 会话文件只存覆盖项（overrides），不存全量——见 scopes.ts。
 */

/** 会话类型（P4 六类 + P8 文件夹合并 + P7 文本合并 UI）：主页大按钮口径 */
export type SessionKind =
  | 'folder-compare'
  | 'text-compare'
  | 'table-compare'
  | 'hex-compare'
  | 'picture-compare'
  | 'folder-sync'
  | 'folder-merge'
  | 'text-merge';

export const SESSION_KINDS: readonly SessionKind[] = [
  'folder-compare',
  'text-compare',
  'table-compare',
  'hex-compare',
  'picture-compare',
  'folder-sync',
  'folder-merge',
  'text-merge',
];

export function isSessionKind(v: unknown): v is SessionKind {
  return typeof v === 'string' && (SESSION_KINDS as readonly string[]).includes(v);
}

/** 会话类型元信息：主页大按钮 / 会话树图标 / 未实现项的阶段标注（FR-HOME-001） */
export interface KindMeta {
  title: string;
  icon: string;
  /** 已实现视图的阶段代号；未实现项注明预期阶段（tooltip） */
  phase: string;
  implemented: boolean;
}

export const KIND_META: Record<SessionKind, KindMeta> = {
  'folder-compare': { title: '文件夹比较', icon: '📁', phase: 'P0', implemented: true },
  'text-compare': { title: '文本比较', icon: '📄', phase: 'P0/P5', implemented: true },
  'table-compare': { title: '表格比较', icon: '📊', phase: 'P0/P14', implemented: true },
  'hex-compare': { title: '十六进制比较', icon: '🔢', phase: 'P0/P14', implemented: true },
  'picture-compare': { title: '图片比较', icon: '🖼️', phase: 'P0/P14', implemented: true },
  'folder-sync': { title: '文件夹同步', icon: '🔄', phase: 'P8', implemented: true },
  'folder-merge': { title: '文件夹合并', icon: '🔀', phase: 'P8', implemented: true },
  'text-merge': { title: '文本合并', icon: '🧩', phase: 'P7/P14', implemented: true },
};

/** 设置作用域（FR-SES-006 四选项 + 子会话专属两项） */
export type SettingsScope = 'session' | 'parent-pair' | 'parent-kind' | 'kind' | 'global';

/**
 * 设置树：按"组.键"嵌套的浅覆盖树（只存覆盖项）。
 * 组对齐设置对话框分页：compare / sync / general / display ...
 */
export type SettingsTree = { [group: string]: { [key: string]: unknown } };

/** 会话节点的规格（对齐 docs/03 §4.3 specs：left/right，将来支持 profile: 前缀） */
export interface SessionSpecs {
  left: string;
  right: string;
  /** 三路会话（folder-merge/text-merge）的公共祖先；两路会话不写该字段 */
  base?: string;
}

/** 会话实例状态（退出时自动保存的那部分；锁定会话跳过写入，FR-SES-001） */
export interface SessionState {
  displayFilter?: string[];
  sort?: { col: string; down: boolean; side: 'left' | 'right' } | null;
  expanded?: string[];
  [key: string]: unknown;
}

/** 类型化会话节点（BCSessions.xml 的 TDirCompareSession 等的 JSON 对应物） */
export interface SessionNode {
  id: string;
  kind: SessionKind;
  name: string;
  locked: boolean;
  createdAt: string;
  updatedAt: string;
  specs: SessionSpecs;
  /** 会话层覆盖项（作用域合并的"会话层"） */
  overrides: SettingsTree;
  state: SessionState;
  /** 所属会话文件夹；null = 根级 */
  folderId: string | null;
}

/** 会话文件夹（树形分组，children 混合持有子文件夹与会话 id） */
export interface SessionFolder {
  id: string;
  name: string;
  children: string[];
}

/** sessions.json 文档（版本化单一 JSON 文档，ADR-005） */
export interface SessionsDoc {
  version: 1;
  folders: SessionFolder[];
  rootFolderIds: string[];
  sessions: SessionNode[];
}

/** session-defaults.json：每类会话的默认设置（FR-SES-003） */
export interface KindDefaultsDoc {
  version: 1;
  kinds: { [k in SessionKind]?: SettingsTree };
}

/** settings.json：全局程序选项 */
export interface GlobalSettingsDoc {
  version: 1;
  settings: SettingsTree;
}

/** state.json：程序状态（BCState.xml 对应物）：MRU + UI 状态 */
export interface StateDoc {
  version: 1;
  mru: {
    /** 最近打开的命名会话 id（先进在前，去重） */
    sessions: string[];
    /** 每类会话的最近路径（BCState 的 LeftMRUSpecs/RightMRUSpecs 对应物） */
    paths: { [k in SessionKind]?: { left: string[]; right: string[] } };
    /** 上限（trim 用，默认 10） */
    max: number;
  };
  ui: {
    showSessionPanel: boolean;
    [key: string]: unknown;
  };
}

// ------------------------------------------------------------------
// id 与时间戳
// ------------------------------------------------------------------

let seq = 0;

/** 会话/文件夹 id：`s-<n>` / `f-<n>`（前缀区分 children 混合列表中的两类节点） */
export function newSessionId(): string {
  return `s-${++seq}-${Date.now().toString(36)}`;
}

export function newFolderId(): string {
  return `f-${++seq}-${Date.now().toString(36)}`;
}

/** 测试与重放用：重置 id 序列（仅测试环境使用） */
export function resetIdSeqForTest(): void {
  seq = 0;
}

function nowIso(): string {
  return new Date().toISOString();
}

// ------------------------------------------------------------------
// 容错解析（损坏文件 → ParseError；.bak 回滚判定在 storage.ts）
// ------------------------------------------------------------------

export class ParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ParseError';
  }
}

export function emptySessionsDoc(): SessionsDoc {
  return { version: 1, folders: [], rootFolderIds: [], sessions: [] };
}

export function emptyKindDefaultsDoc(): KindDefaultsDoc {
  return { version: 1, kinds: {} };
}

export function emptyGlobalSettingsDoc(): GlobalSettingsDoc {
  return { version: 1, settings: {} };
}

export function emptyStateDoc(): StateDoc {
  return { version: 1, mru: { sessions: [], paths: {}, max: 10 }, ui: { showSessionPanel: true } };
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** 规范化 specs：两侧均为非空字符串；缺失补空串（保存会话前由 UI 校验非空） */
function normalizeSpecs(raw: unknown): SessionSpecs {
  const r = isRecord(raw) ? raw : {};
  const specs: SessionSpecs = {
    left: typeof r.left === 'string' ? r.left : '',
    right: typeof r.right === 'string' ? r.right : '',
  };
  if (typeof r.base === 'string' && r.base !== '') specs.base = r.base;
  return specs;
}

function normalizeOverrides(raw: unknown): SettingsTree {
  if (!isRecord(raw)) return {};
  const out: SettingsTree = {};
  for (const [group, value] of Object.entries(raw)) {
    if (isRecord(value)) out[group] = { ...value };
  }
  return out;
}

function normalizeState(raw: unknown): SessionState {
  return isRecord(raw) ? { ...raw } : {};
}

/**
 * 解析 sessions.json。容错策略：字段缺失/类型不符按默认值补齐（不抛错），
 * 仅当顶层结构完全不可用（非对象 / version 未知且无 sessions 数组）抛 ParseError，
 * 以便上层走 .bak 回滚（R12 演练）。
 */
export function parseSessionsDoc(text: string): SessionsDoc {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new ParseError('sessions.json 不是合法 JSON');
  }
  if (!isRecord(raw)) throw new ParseError('sessions.json 顶层不是对象');
  if (raw.version !== 1)
    throw new ParseError(`不支持的 sessions.json 版本: ${String(raw.version)}`);

  const folders: SessionFolder[] = [];
  if (Array.isArray(raw.folders)) {
    for (const f of raw.folders) {
      if (!isRecord(f) || typeof f.id !== 'string') continue;
      folders.push({
        id: f.id,
        name: typeof f.name === 'string' ? f.name : '未命名',
        children: Array.isArray(f.children)
          ? f.children.filter((c): c is string => typeof c === 'string')
          : [],
      });
    }
  }
  const sessions: SessionNode[] = [];
  if (Array.isArray(raw.sessions)) {
    for (const s of raw.sessions) {
      if (!isRecord(s) || typeof s.id !== 'string' || !isSessionKind(s.kind)) continue;
      sessions.push({
        id: s.id,
        kind: s.kind,
        name: typeof s.name === 'string' && s.name !== '' ? s.name : '未命名会话',
        locked: s.locked === true,
        createdAt: typeof s.createdAt === 'string' ? s.createdAt : nowIso(),
        updatedAt: typeof s.updatedAt === 'string' ? s.updatedAt : nowIso(),
        specs: normalizeSpecs(s.specs),
        overrides: normalizeOverrides(s.overrides),
        state: normalizeState(s.state),
        folderId: typeof s.folderId === 'string' ? s.folderId : null,
      });
    }
  }
  const knownIds = new Set([...folders.map((f) => f.id), ...sessions.map((s) => s.id)]);
  const rootFolderIds = Array.isArray(raw.rootFolderIds)
    ? raw.rootFolderIds.filter((id): id is string => typeof id === 'string' && knownIds.has(id))
    : [];
  // 根缺省兜底：未被任何文件夹持有的节点视作根（防旧版本/手改文件丢节点）
  const held = new Set<string>();
  for (const f of folders) for (const c of f.children) held.add(c);
  for (const s of sessions)
    if (s.folderId && !folders.some((f) => f.id === s.folderId)) s.folderId = null;
  if (rootFolderIds.length === 0) {
    rootFolderIds.push(...folders.map((f) => f.id).filter((id) => !held.has(id)));
  }
  return { version: 1, folders, rootFolderIds, sessions };
}

export function parseKindDefaultsDoc(text: string): KindDefaultsDoc {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new ParseError('session-defaults.json 不是合法 JSON');
  }
  if (!isRecord(raw) || raw.version !== 1) throw new ParseError('session-defaults.json 版本不支持');
  const kinds: KindDefaultsDoc['kinds'] = {};
  if (isRecord(raw.kinds)) {
    for (const [k, v] of Object.entries(raw.kinds)) {
      if (isSessionKind(k)) kinds[k] = normalizeOverrides(v);
    }
  }
  return { version: 1, kinds };
}

export function parseGlobalSettingsDoc(text: string): GlobalSettingsDoc {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new ParseError('settings.json 不是合法 JSON');
  }
  if (!isRecord(raw) || raw.version !== 1) throw new ParseError('settings.json 版本不支持');
  return { version: 1, settings: normalizeOverrides(raw.settings) };
}

export function parseStateDoc(text: string): StateDoc {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new ParseError('state.json 不是合法 JSON');
  }
  if (!isRecord(raw) || raw.version !== 1) throw new ParseError('state.json 版本不支持');
  const mruRaw = isRecord(raw.mru) ? raw.mru : {};
  const paths: StateDoc['mru']['paths'] = {};
  if (isRecord(mruRaw.paths)) {
    for (const [k, v] of Object.entries(mruRaw.paths)) {
      if (!isSessionKind(k) || !isRecord(v)) continue;
      const toList = (x: unknown): string[] =>
        Array.isArray(x) ? x.filter((s): s is string => typeof s === 'string') : [];
      paths[k] = { left: toList(v.left), right: toList(v.right) };
    }
  }
  const uiRaw = isRecord(raw.ui) ? raw.ui : {};
  return {
    version: 1,
    mru: {
      sessions: Array.isArray(mruRaw.sessions)
        ? mruRaw.sessions.filter((s): s is string => typeof s === 'string')
        : [],
      paths,
      max: typeof mruRaw.max === 'number' && mruRaw.max > 0 ? mruRaw.max : 10,
    },
    ui: { showSessionPanel: uiRaw.showSessionPanel !== false },
  };
}

// ------------------------------------------------------------------
// 序列化（稳定输出：2 空格缩进 + 末尾换行；原子写见 storage.ts）
// ------------------------------------------------------------------

export function serializeDoc(doc: unknown): string {
  return `${JSON.stringify(doc, null, 2)}\n`;
}
