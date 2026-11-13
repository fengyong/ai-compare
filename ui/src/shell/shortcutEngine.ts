import { DEFAULT_BINDINGS, eventMatches, formatBinding, parseBinding, findConflicts, type KeyBinding } from './shortcuts';
import { commandRegistry, type CommandContext, type CommandRegistry } from './commands';

/**
 * 快捷键引擎（FR-UI-003/004）：全局 keydown → 命令解析 → 执行。
 *
 * - 键位表：默认 BC 键位（DEFAULT_BINDINGS，docs/01 附录 G）+ 用户覆盖（重绑定）；
 * - 冲突检测：重绑定时提示重新分配（同键位占用多个命令 → findConflicts）；
 * - 输入保护：焦点在输入框/文本域时，无修饰键的单键绑定（如 Space）不触发。
 */

export interface ShortcutEngineOptions {
  registry?: CommandRegistry;
  /** 构建命令运行上下文（壳在触发时调用） */
  buildContext(): CommandContext;
  /** 用户自定义覆盖（commandId → 绑定串；缺省命令用 DEFAULT_BINDINGS） */
  overrides?: Record<string, string>;
  /** 平台判定覆盖（测试用；缺省按 navigator.platform 检测，mac 上 Ctrl≈Cmd） */
  isMacOverride?: boolean;
  /** 重绑定发生冲突时的回调（返回 false = 拒绝重绑定） */
  onConflict?(conflicts: BindingConflictEx[]): boolean;
}

export interface BindingConflictEx {
  binding: string;
  commands: string[];
  incoming: string;
}

export class ShortcutEngine {
  private overrides: Record<string, string>;
  private readonly handler: (e: KeyboardEvent) => void;
  private attached = false;

  constructor(private readonly opts: ShortcutEngineOptions) {
    this.overrides = { ...(opts.overrides ?? {}) };
    this.handler = (e) => this.handleKey(e);
  }

  /** 生效键位表（命令 id → 绑定串；默认 BC 键位 + 注册命令键位 + 用户覆盖） */
  bindings(): Record<string, string> {
    const out = { ...(this.opts.registry?.defaultBindings() ?? DEFAULT_BINDINGS) };
    Object.assign(out, this.overrides);
    return out;
  }

  attach(): void {
    if (this.attached || typeof window === 'undefined') return;
    window.addEventListener('keydown', this.handler, true);
    this.attached = true;
  }

  detach(): void {
    if (!this.attached) return;
    window.removeEventListener('keydown', this.handler, true);
    this.attached = false;
  }

  /** 重绑定命令快捷键（冲突基于全量生效键位表检测；冲突时回调确认，拒绝则不变） */
  rebind(commandId: string, keys: string): boolean {
    const parsed = parseBinding(keys);
    if (!parsed) return false;
    const next = { ...this.overrides, [commandId]: keys };
    const effective = { ...(this.opts.registry?.defaultBindings() ?? {}), ...next };
    const conflicts = findConflicts(effective).filter(
      (c) => c.commands.includes(commandId) && c.commands.length > 1
    );
    if (conflicts.length > 0) {
      const ok = this.opts.onConflict?.(
        conflicts.map((c) => ({ ...c, incoming: formatBinding(parsed) }))
      );
      if (!ok) return false;
    }
    this.overrides = next;
    return true;
  }

  getOverrides(): Record<string, string> {
    return { ...this.overrides };
  }

  /** 命中事件 → 命令 id（输入焦点保护内联于此） */
  resolveCommand(e: KeyboardEvent): string | null {
    const bindings = this.bindings();
    for (const [cmdId, spec] of Object.entries(bindings)) {
      const b = parseBinding(spec);
      if (b && this.matches(e, b)) return cmdId;
    }
    return null;
  }

  private matches(e: KeyboardEvent, b: KeyBinding): boolean {
    return eventMatches(e, b, this.opts.isMacOverride);
  }

  private handleKey(e: KeyboardEvent): void {
    const target = e.target as HTMLElement | null;
    const inEditor =
      target &&
      (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);
    if (inEditor) {
      const b = this.firstMatchingBinding(e);
      if (b && !b.ctrl && !b.alt) return; // 单键在输入框内保持打字语义
    }
    const cmdId = this.resolveCommand(e);
    if (!cmdId) return;
    const def = this.opts.registry?.get(cmdId);
    if (!def) return;
    if (def.when && !def.when(this.opts.buildContext())) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      const result = def.run(this.opts.buildContext());
      if (result instanceof Promise) {
        result.catch((err) => this.opts.buildContext().notice(`命令执行失败: ${String(err)}`));
      }
    } catch (err) {
      this.opts.buildContext().notice(`命令执行失败: ${String(err)}`);
    }
  }

  private firstMatchingBinding(e: KeyboardEvent): { ctrl: boolean; alt: boolean } | null {
    for (const spec of Object.values(this.bindings())) {
      const b = parseBinding(spec);
      if (b && this.matches(e, b)) return b;
    }
    return null;
  }

  /** 调试/测试：直接运行命令 */
  runCommand(cmdId: string): void {
    const def = this.opts.registry?.get(cmdId);
    if (!def) return;
    if (def.when && !def.when(this.opts.buildContext())) return;
    void def.run(this.opts.buildContext());
  }
}

/** 应用级单例（AppShell 挂载时 attach） */
export function createShortcutEngine(opts: ShortcutEngineOptions): ShortcutEngine {
  if (!opts.registry) {
    opts.registry = commandRegistry;
  }
  return new ShortcutEngine(opts);
}
