import type {
  FileFormatSpec,
  FormatPackSpec,
  FormatViewKind,
  GrammarElementSpec,
  GrammarItemSpec,
} from '../../protocol/types';
import { looksTextual } from '../../lib/env';

/**
 * 文件格式页的纯逻辑层（P10 / FR-FMT-001~003 设置侧）。
 *
 * 职责：
 * - 内置格式目录（与引擎 ai-core formats.rs builtin 表同源的 UI 镜像；
 *   id 必须一致——运行时引擎注册表为权威，本目录供设置编辑/元素勾选/掩码预览）；
 * - 持久化状态形态（settings.json 全局层 `fileFormats` 组）：
 *   `{ custom: FileFormatSpec[], builtinOverrides: { [id]: { enabled?, masks? } } }`；
 * - BC 掩码语义（FR-FLT-001，与引擎 mask_matches 同构）；
 * - 格式列表语义：顺序敏感、首匹配生效、禁用跳过、遮蔽检测（红显）；
 * - 格式集导入导出（BC 风格 .bcpack / JSON 载体，FormatPackSpec）。
 *
 * 全部纯函数（无 React / 无 IO），供组件与单测共用。
 */

/** 设置树中 fileFormats 组的持久化形态 */
export interface FileFormatsState {
  /** 用户自定义格式（追加在内置表之后） */
  custom: FileFormatSpec[];
  /** 内置格式覆盖（enabled / masks；其余字段不可覆盖） */
  builtinOverrides: { [id: string]: { enabled?: boolean; masks?: string[] } };
}

// ---------------------------------------------------------------------------
// 语法项便捷构造（内置目录与"新增格式"默认元素用）
// ---------------------------------------------------------------------------

export function lineCommentItem(start: string): GrammarItemSpec {
  return {
    kind: 'delimited',
    tokens: [],
    start,
    toEol: true,
    regex: false,
    caseSensitive: false,
    orFirstLine: false,
  };
}

export function blockCommentItem(start: string, end: string): GrammarItemSpec {
  return { ...lineCommentItem(start), toEol: false, end };
}

export function stringItem(quote: string, escape = '\\'): GrammarItemSpec {
  return { ...lineCommentItem(quote), toEol: false, end: quote, escape };
}

function element(name: string, items: GrammarItemSpec[]): GrammarElementSpec {
  return { name, caseSensitive: false, items };
}

// ---------------------------------------------------------------------------
// 内置格式目录（引擎 ai-core::formats::builtin_formats 的 UI 镜像；顺序即优先级）
// ---------------------------------------------------------------------------

const C_FAMILY = (): GrammarElementSpec[] => [
  element('注释', [blockCommentItem('/*', '*/'), lineCommentItem('//')]),
  element('字符串', [stringItem('"'), stringItem("'")]),
];

export const BUILTIN_FORMAT_CATALOG: FileFormatSpec[] = [
  {
    id: 'builtin-cpp',
    enabled: true,
    masks: ['*.cpp', '*.cc', '*.cxx', '*.hpp', '*.hh', '*.hxx', '*.h', '*.c'],
    description: 'C/C++ 源码',
    viewKind: 'text',
    grammar: C_FAMILY(),
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-csharp',
    enabled: true,
    masks: ['*.cs'],
    description: 'C# 源码',
    viewKind: 'text',
    grammar: C_FAMILY(),
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-java',
    enabled: true,
    masks: ['*.java'],
    description: 'Java 源码',
    viewKind: 'text',
    grammar: C_FAMILY(),
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-jsts',
    enabled: true,
    masks: ['*.js', '*.mjs', '*.cjs', '*.jsx', '*.ts', '*.tsx', '*.mts', '*.cts'],
    description: 'JavaScript/TypeScript',
    viewKind: 'text',
    grammar: [
      element('注释', [blockCommentItem('/*', '*/'), lineCommentItem('//')]),
      element('字符串', [stringItem('"'), stringItem("'"), stringItem('`')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-python',
    enabled: true,
    masks: ['*.py', '*.pyw', '*.pyi'],
    description: 'Python 源码',
    viewKind: 'text',
    grammar: [
      element('注释', [lineCommentItem('#')]),
      element('字符串', [stringItem('"""'), stringItem("'''"), stringItem('"'), stringItem("'")]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-go',
    enabled: true,
    masks: ['*.go'],
    description: 'Go 源码',
    viewKind: 'text',
    grammar: [
      element('注释', [blockCommentItem('/*', '*/'), lineCommentItem('//')]),
      element('字符串', [stringItem('"'), stringItem('`', '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-rust',
    enabled: true,
    masks: ['*.rs'],
    description: 'Rust 源码',
    viewKind: 'text',
    grammar: [
      element('注释', [blockCommentItem('/*', '*/'), lineCommentItem('//')]),
      element('字符串', [stringItem('"')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-sql',
    enabled: true,
    masks: ['*.sql'],
    description: 'SQL 脚本',
    viewKind: 'text',
    grammar: [
      element('注释', [lineCommentItem('--'), blockCommentItem('/*', '*/')]),
      element('字符串', [stringItem("'", ''), stringItem('"', '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-sh',
    enabled: true,
    masks: ['*.sh', '*.bash', '*.zsh'],
    description: 'Shell 脚本',
    viewKind: 'text',
    grammar: [
      element('注释', [lineCommentItem('#')]),
      element('字符串', [stringItem('"'), stringItem("'", '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-perl',
    enabled: true,
    masks: ['*.pl', '*.pm'],
    description: 'Perl 源码',
    viewKind: 'text',
    grammar: [
      element('注释', [lineCommentItem('#')]),
      element('字符串', [stringItem('"'), stringItem("'", '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-ruby',
    enabled: true,
    masks: ['*.rb'],
    description: 'Ruby 源码',
    viewKind: 'text',
    grammar: [
      element('注释', [lineCommentItem('#')]),
      element('字符串', [stringItem('"'), stringItem("'", '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-lua',
    enabled: true,
    masks: ['*.lua'],
    description: 'Lua 源码',
    viewKind: 'text',
    grammar: [
      element('注释', [blockCommentItem('--[[', ']]'), lineCommentItem('--')]),
      element('字符串', [stringItem('"')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-xml',
    enabled: true,
    masks: ['*.xml', '*.xsl', '*.xslt', '*.svg', '*.xsd'],
    description: 'XML 文档',
    viewKind: 'text',
    grammar: [
      element('注释', [blockCommentItem('<!--', '-->')]),
      element('字符串', [stringItem('"', ''), stringItem("'", '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-json',
    enabled: true,
    masks: ['*.json', '*.jsonc', '*.json5'],
    description: 'JSON 数据',
    viewKind: 'text',
    grammar: [
      element('注释', [blockCommentItem('/*', '*/'), lineCommentItem('//')]),
      element('字符串', [stringItem('"')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-toml',
    enabled: true,
    masks: ['*.toml'],
    description: 'TOML 配置',
    viewKind: 'text',
    grammar: [
      element('注释', [lineCommentItem('#')]),
      element('字符串', [stringItem('"""'), stringItem('"'), stringItem("'", '')]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-yaml',
    enabled: true,
    masks: ['*.yml', '*.yaml'],
    description: 'YAML 配置',
    viewKind: 'text',
    grammar: [element('注释', [lineCommentItem('#')])],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-ini',
    enabled: true,
    masks: ['*.ini', '*.cfg', '*.conf', '*.properties'],
    description: 'INI 配置',
    viewKind: 'text',
    grammar: [element('注释', [lineCommentItem(';'), lineCommentItem('#')])],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-report',
    enabled: true,
    masks: ['*.prt', '*.listing'],
    description: '分页报表文本',
    viewKind: 'text',
    grammar: [
      element('页标题', [
        {
          kind: 'rowTitle',
          pattern: '\\f.*',
          tokens: [],
          toEol: false,
          regex: false,
          caseSensitive: false,
          lines: 2,
          orFirstLine: true,
        },
      ]),
    ],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  // 非文本视图格式（FR-FMT-004）：与引擎 builtin_formats 同 id/同掩码/同顺序。
  // 双击目录行的视图类型由本表首匹配决定（FR-SEL-001）。
  {
    id: 'builtin-table',
    enabled: true,
    masks: ['*.csv', '*.tsv', '*.tab'],
    description: '表格数据（CSV/制表符分隔）',
    viewKind: 'table',
    grammar: [],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-hex',
    enabled: true,
    masks: [
      '*.exe',
      '*.dll',
      '*.so',
      '*.dylib',
      '*.bin',
      '*.dat',
      '*.o',
      '*.obj',
      '*.a',
      '*.lib',
      '*.class',
    ],
    description: '二进制/可执行文件',
    viewKind: 'hex',
    grammar: [],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
  {
    id: 'builtin-picture',
    enabled: true,
    masks: ['*.gif', '*.ico', '*.jpeg', '*.jpg', '*.png', '*.tiff', '*.tif', '*.bmp'],
    description: '图片',
    viewKind: 'picture',
    grammar: [],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  },
];

export function defaultFileFormatsState(): FileFormatsState {
  return { custom: [], builtinOverrides: {} };
}

// ---------------------------------------------------------------------------
// 掩码语义（FR-FLT-001；与引擎 mask_matches 同构）
// ---------------------------------------------------------------------------

/**
 * BC 文件名掩码匹配：
 * - `?` 任意单字符；`*` 零或任意数量字符（含 `.`）；`*.*`/`*` 匹配一切；
 * - `[az]`/`[a-z]` 集合/范围；`[!az]`/`[^az]` 补集；
 * - 掩码以 `.` 结尾时仅匹配无扩展名文件且通配符不吞句点；
 * - 大小写不敏感。
 */
export function maskMatches(mask: string, name: string): boolean {
  const m = mask.trim().toLowerCase();
  const n = name.toLowerCase();
  if (m === '*.*' || m === '*') return true;
  if (m.endsWith('.')) {
    if (n.includes('.')) return false;
    return globMatch(m.slice(0, -1), n, true);
  }
  return globMatch(m, n, false);
}

function globMatch(pattern: string, name: string, dotFree: boolean): boolean {
  const p = [...pattern];
  const t = [...name];
  let pi = 0;
  let ni = 0;
  let starPi = -1;
  let starNi = 0;
  while (ni < t.length) {
    if (pi < p.length && p[pi] === '*') {
      starPi = pi;
      starNi = ni;
      pi += 1;
      continue;
    }
    let ok = false;
    if (pi < p.length) {
      const next = globChar(p, pi, t[ni], dotFree);
      if (next >= 0) {
        pi = next;
        ok = true;
      }
    }
    if (ok) {
      ni += 1;
    } else if (starPi >= 0) {
      if (dotFree && t[starNi] === '.') return false;
      starNi += 1;
      pi = starPi + 1;
      ni = starNi;
    } else {
      return false;
    }
  }
  while (pi < p.length && p[pi] === '*') pi += 1;
  return pi === p.length;
}

/** 单字符原子匹配；命中返回原子后的下标，未命中返回 -1 */
function globChar(p: string[], pi: number, c: string, dotFree: boolean): number {
  const pc = p[pi];
  if (pc === '?') {
    if (dotFree && c === '.') return -1;
    return pi + 1;
  }
  if (pc === '[') {
    let i = pi + 1;
    const neg = p[i] === '^' || p[i] === '!';
    if (neg) i += 1;
    let matched = false;
    let first = true;
    while (i < p.length && (first || p[i] !== ']')) {
      first = false;
      const lo = p[i];
      if (i + 2 < p.length && p[i + 1] === '-' && p[i + 2] !== ']') {
        if (c >= lo && c <= p[i + 2]) matched = true;
        i += 3;
      } else {
        if (c === lo) matched = true;
        i += 1;
      }
    }
    if (i >= p.length) return -1; // 未闭合类按不匹配
    return matched !== neg ? i + 1 : -1;
  }
  return pc === c ? pi + 1 : -1;
}

/** 掩码编辑框文本 -> 掩码数组（`;`/`,` 分隔、去空、去重） */
export function parseMaskList(text: string): string[] {
  const out: string[] = [];
  for (const part of text.split(/[;,]/)) {
    const m = part.trim();
    if (m && !out.includes(m)) out.push(m);
  }
  return out;
}

/** 掩码数组 -> 编辑框文本 */
export function maskListText(masks: string[]): string {
  return masks.join('; ');
}

// ---------------------------------------------------------------------------
// 生效格式列表（顺序敏感 + 遮蔽检测）
// ---------------------------------------------------------------------------

/** 生效格式列表：内置（套用 overrides）在前、自定义在后；顺序即匹配优先级 */
export function effectiveFormats(state: FileFormatsState): FileFormatSpec[] {
  const builtins = BUILTIN_FORMAT_CATALOG.map((f) => {
    const o = state.builtinOverrides[f.id];
    if (!o) return f;
    return {
      ...f,
      enabled: o.enabled ?? f.enabled,
      masks: o.masks ?? f.masks,
    };
  });
  return [...builtins, ...state.custom];
}

/** 首个掩码匹配且启用的格式（FR-FMT-001；空掩码不参与） */
export function firstMatchingFormat(
  formats: FileFormatSpec[],
  fileName: string,
): FileFormatSpec | null {
  for (const f of formats) {
    if (!f.enabled || f.masks.length === 0) continue;
    if (f.masks.some((m) => maskMatches(m, fileName))) return f;
  }
  return null;
}

/**
 * 打开文件时的视图类型（FR-DIR-070 / FR-SEL-001）：格式列表自上而下首匹配生效。
 * 无匹配 → 默认文本比较；扩展名不在文本表内时按内部启发兜底十六进制
 * （FR-SEL-001「无匹配格式 → 默认文本比较（或十六进制，按内部启发）」）。
 */
export function resolveViewKind(formats: FileFormatSpec[], fileName: string): FormatViewKind {
  const fmt = firstMatchingFormat(formats, fileName);
  if (fmt) return fmt.viewKind;
  return looksTextual(fileName) ? 'text' : 'hex';
}

/**
 * 被遮蔽的格式 id 集（UI 红显）：更靠前的启用格式含 全匹配（`*`/`*.*`）
 * 或与之完全相同的掩码。
 */
export function shadowedFormatIds(formats: FileFormatSpec[]): Set<string> {
  const out = new Set<string>();
  for (let i = 0; i < formats.length; i++) {
    const f = formats[i];
    if (!f.enabled || f.masks.length === 0) continue;
    const shadowed = formats.slice(0, i).some((earlier) => {
      if (!earlier.enabled || earlier.masks.length === 0) return false;
      return earlier.masks.some((m) => {
        const t = m.trim();
        if (t === '*.*' || t === '*') return true;
        return f.masks.some((o) => o.trim() === t);
      });
    });
    if (shadowed) out.add(f.id);
  }
  return out;
}

// ---------------------------------------------------------------------------
// 状态变更（纯函数：返回新 state）
// ---------------------------------------------------------------------------

export function upsertCustom(state: FileFormatsState, fmt: FileFormatSpec): FileFormatsState {
  const exists = state.custom.some((f) => f.id === fmt.id);
  const custom = exists
    ? state.custom.map((f) => (f.id === fmt.id ? fmt : f))
    : [...state.custom, fmt];
  return { ...state, custom };
}

export function removeCustom(state: FileFormatsState, id: string): FileFormatsState {
  return { ...state, custom: state.custom.filter((f) => f.id !== id) };
}

export function setBuiltinOverride(
  state: FileFormatsState,
  id: string,
  patch: { enabled?: boolean; masks?: string[] },
): FileFormatsState {
  const prev = state.builtinOverrides[id] ?? {};
  return {
    ...state,
    builtinOverrides: { ...state.builtinOverrides, [id]: { ...prev, ...patch } },
  };
}

/** 新自定义格式的默认定义（一个 // 行注释元素；掩码留空待填） */
export function newCustomFormat(id: string): FileFormatSpec {
  return {
    id,
    enabled: true,
    masks: [],
    description: '',
    viewKind: 'text',
    grammar: [element('注释', [lineCommentItem('//')])],
    tabStops: 4,
    independentLines: false,
    columnData: false,
  };
}

/** 自定义格式元素编辑：按元素下标替换（name/kind/pattern 三件套的最小编排） */
export function updateCustomElement(
  state: FileFormatsState,
  formatId: string,
  elementIndex: number,
  patch: { name?: string; kind?: GrammarItemSpec['kind']; pattern?: string },
): FileFormatsState {
  return {
    ...state,
    custom: state.custom.map((f) => {
      if (f.id !== formatId) return f;
      const grammar = f.grammar.map((el, i) => {
        if (i !== elementIndex) return el;
        const item = el.items[0] ?? lineCommentItem('');
        const nextItem: GrammarItemSpec = {
          ...item,
          kind: patch.kind ?? item.kind,
          pattern: patch.pattern ?? item.pattern,
        };
        return { ...el, name: patch.name ?? el.name, items: [nextItem, ...el.items.slice(1)] };
      });
      return { ...f, grammar };
    }),
  };
}

// ---------------------------------------------------------------------------
// 格式集导入导出（BC 风格 .bcpack / JSON）
// ---------------------------------------------------------------------------

export type ImportResult =
  { ok: true; loaded: number; state: FileFormatsState } | { ok: false; error: string };

/** 解析并合并格式集：内置 id 落到 overrides（enabled/masks），其余进 custom */
export function importFormatPack(state: FileFormatsState, json: string): ImportResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return { ok: false, error: '格式集不是合法 JSON' };
  }
  const pack = parsed as Partial<FormatPackSpec>;
  if (!pack || typeof pack !== 'object' || !Array.isArray(pack.formats)) {
    return { ok: false, error: '格式集缺少 formats 数组' };
  }
  if (typeof pack.version === 'number' && pack.version > 1) {
    return { ok: false, error: `格式集版本不支持: ${pack.version}` };
  }
  let next = state;
  let loaded = 0;
  for (const raw of pack.formats) {
    const fmt = normalizeFormatSpec(raw);
    if (!fmt) continue;
    loaded += 1;
    if (fmt.id.startsWith('builtin-')) {
      next = setBuiltinOverride(next, fmt.id, { enabled: fmt.enabled, masks: fmt.masks });
    } else {
      next = upsertCustom(next, fmt);
    }
  }
  return { ok: true, loaded, state: next };
}

/** 把任意对象规整为 FileFormatSpec（宽松：缺省字段补齐；id 缺失丢弃） */
export function normalizeFormatSpec(raw: unknown): FileFormatSpec | null {
  if (!raw || typeof raw !== 'object') return null;
  const o = raw as Partial<FileFormatSpec>;
  if (typeof o.id !== 'string' || o.id.length === 0) return null;
  return {
    id: o.id,
    enabled: o.enabled ?? true,
    masks: Array.isArray(o.masks) ? o.masks.filter((m): m is string => typeof m === 'string') : [],
    description: typeof o.description === 'string' ? o.description : '',
    viewKind: isViewKind(o.viewKind) ? o.viewKind : 'text',
    grammar: Array.isArray(o.grammar) ? o.grammar : [],
    tabStops: typeof o.tabStops === 'number' ? o.tabStops : 4,
    independentLines: o.independentLines ?? false,
    columnData: o.columnData ?? false,
  };
}

function isViewKind(v: unknown): v is FormatViewKind {
  return v === 'text' || v === 'table' || v === 'hex' || v === 'picture' || v === 'external';
}

/** 导出当前生效全量（内置套 overrides + custom）为格式集 JSON（pretty） */
export function exportFormatPack(state: FileFormatsState): string {
  const pack: FormatPackSpec = {
    version: 1,
    kind: 'ai-compare-format-pack',
    formats: effectiveFormats(state),
  };
  return JSON.stringify(pack, null, 2);
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

/** 保存前校验（返回错误列表；空 = 合法） */
export function validateFormatSpec(fmt: FileFormatSpec): string[] {
  const errors: string[] = [];
  if (!fmt.id.trim()) errors.push('格式 id 不能为空');
  if (fmt.id.startsWith('builtin-')) errors.push('自定义格式 id 不能以 builtin- 开头');
  if (fmt.masks.length === 0) errors.push('至少需要一个掩码（空掩码不匹配任何文件）');
  for (const m of fmt.masks) {
    if (!m.trim()) errors.push('掩码不能为空白项');
    else if (/[\r\n]/.test(m)) errors.push('掩码不能包含换行');
  }
  return errors;
}
