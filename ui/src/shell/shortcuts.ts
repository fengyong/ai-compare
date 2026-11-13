/**
 * 快捷键引擎（FR-UI-003/004：全命令可绑定、冲突检测；默认键位对齐 docs/01 附录 G）。
 *
 * 绑定串格式：`Ctrl+Shift+K` / `Alt+F5` / `Space`（macOS 上 Ctrl 命令按 Cmd 匹配，
 * 附录 G 的 Ctrl+L/Ctrl+R 等惯例在 mac 对应 Cmd+L/Cmd+R）。
 * 冲突检测在两处使用：①默认键位表构建时自检；②用户自定义重绑定时提示重新分配。
 */

export interface KeyBinding {
  ctrl: boolean; // macOS 上匹配 Meta（Cmd）
  alt: boolean;
  shift: boolean;
  /** 主键（小写；单字符或 'f1'~'f12'、'space'、'tab'、'enter'、'escape'、'delete' 等命名键） */
  key: string;
}

const NAMED_KEYS = new Set([
  'space', 'tab', 'enter', 'escape', 'backspace', 'delete', 'insert',
  'home', 'end', 'pageup', 'pagedown', 'left', 'right', 'up', 'down',
]);

function normalizeKeyName(raw: string): string {
  const k = raw.trim().toLowerCase();
  if (k === 'del') return 'delete';
  if (k === 'esc') return 'escape';
  if (k === 'return') return 'enter';
  if (k === 'spacebar') return 'space';
  return k;
}

/** 解析绑定串；非法输入（空、无主键、未知分隔）返回 null */
export function parseBinding(spec: string): KeyBinding | null {
  if (typeof spec !== 'string') return null;
  const parts = spec.split('+').map((p) => p.trim()).filter((p) => p !== '');
  if (parts.length === 0) return null;
  const key = normalizeKeyName(parts[parts.length - 1]);
  if (!key) return null;
  const isSingleChar = key.length === 1;
  if (!isSingleChar && !/^(f[1-9]|f1[0-2])$/.test(key) && !NAMED_KEYS.has(key)) return null;
  let ctrl = false;
  let alt = false;
  let shift = false;
  for (const p of parts.slice(0, -1)) {
    const mod = p.toLowerCase();
    if (mod === 'ctrl' || mod === 'cmd' || mod === 'meta' || mod === 'control') ctrl = true;
    else if (mod === 'alt' || mod === 'option') alt = true;
    else if (mod === 'shift') shift = true;
    else return null; // 未知修饰符
  }
  return { ctrl, alt, shift, key };
}

/** 反序列化为规范显示串（Ctrl+Alt+C 风格，修饰符有序） */
export function formatBinding(b: KeyBinding): string {
  const parts: string[] = [];
  if (b.ctrl) parts.push('Ctrl');
  if (b.alt) parts.push('Alt');
  if (b.shift) parts.push('Shift');
  parts.push(b.key.length === 1 ? b.key.toUpperCase() : b.key);
  return parts.join('+');
}

/** 归一化签名（冲突分组的键） */
export function bindingSignature(b: KeyBinding): string {
  return formatBinding(b);
}

/** KeyboardEvent 是否命中绑定（Ctrl 在 mac 上按 Cmd 匹配，其余平台按 ctrlKey） */
export function eventMatches(
  e: {
    key: string;
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
    metaKey: boolean;
  },
  b: KeyBinding,
  isMacOverride?: boolean
): boolean {
  const isMac =
    isMacOverride ??
    (typeof navigator !== 'undefined' && /mac/i.test(navigator.platform || navigator.userAgent || ''));
  const ctrlHit = isMac ? e.metaKey : e.ctrlKey;
  const primaryKey = e.key === ' ' ? 'space' : normalizeKeyName(e.key);
  return (
    ctrlHit === b.ctrl && e.altKey === b.alt && e.shiftKey === b.shift && primaryKey === b.key
  );
}

export interface BindingConflict {
  /** 冲突的键位串 */
  binding: string;
  /** 共用该键位的命令 id 列表 */
  commands: string[];
}

/**
 * 冲突检测：同一键位被多个命令占用 → 返回冲突组。
 * 用户重绑定时据此提示"重新分配"（FR-UI-003）。
 */
export function findConflicts(bindings: Record<string, string>): BindingConflict[] {
  const byBinding = new Map<string, string[]>();
  for (const [cmd, spec] of Object.entries(bindings)) {
    const b = parseBinding(spec);
    if (!b) continue; // 非法绑定不参与冲突判定（注册时报错）
    const sig = bindingSignature(b);
    const list = byBinding.get(sig);
    if (list) list.push(cmd);
    else byBinding.set(sig, [cmd]);
  }
  const out: BindingConflict[] = [];
  for (const [binding, commands] of byBinding) {
    if (commands.length > 1) out.push({ binding, commands: [...commands].sort() });
  }
  return out.sort((a, b) => a.binding.localeCompare(b.binding));
}

/**
 * 默认键位表（docs/01 附录 G「默认键盘快捷键与交互惯例」明示部分 + 壳级惯例）：
 * 仅收录本阶段已有命令；视图专属键（空格标记选择、Shift+方向限制侧）由
 * 目录视图在 P6 接管（键位先登记占位，命令缺省为 no-op 提示）。
 */
export const DEFAULT_BINDINGS: Record<string, string> = {
  // 附录 G 明示
  'folder.copyToLeft': 'Ctrl+L',
  'folder.copyToRight': 'Ctrl+R',
  'app.clipboardCompare': 'Ctrl+Alt+C',
  'folder.markSelection': 'Space',
  // 壳级惯例（对齐 BC：标签页/窗口/会话命令）
  'app.newTab': 'Ctrl+T',
  'app.newWindow': 'Ctrl+N',
  'app.closeTab': 'Ctrl+W',
  'window.mergeAll': 'Ctrl+Alt+M',
  'window.detachTab': 'Ctrl+Alt+N',
  'tab.next': 'Ctrl+PageDown',
  'tab.prev': 'Ctrl+PageUp',
  'session.save': 'Ctrl+S',
  'session.saveAs': 'Ctrl+Alt+S',
  'session.lock': 'Ctrl+Alt+L',
  'app.settings': 'Ctrl+K',
  'app.showSessionPanel': 'F9',
};

/** 默认键位表自检：构建期即断言无冲突（冲突检测的单测同时覆盖） */
export function defaultBindingConflicts(): BindingConflict[] {
  return findConflicts(DEFAULT_BINDINGS);
}
