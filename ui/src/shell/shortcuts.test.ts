import { describe, it, expect } from 'vitest';
import {
  DEFAULT_BINDINGS,
  defaultBindingConflicts,
  eventMatches,
  findConflicts,
  formatBinding,
  parseBinding,
} from './shortcuts';
import { commandRegistry } from './commands';
import { createShortcutEngine, ShortcutEngine } from './shortcutEngine';
import type { CommandContext } from './commands';
import { registerFolderView } from '../views/folder-compare/folderViewBus';

function fakeEvent(key: string, mods: Partial<{ ctrl: boolean; alt: boolean; shift: boolean; meta: boolean }> = {}): KeyboardEvent {
  return {
    key,
    ctrlKey: mods.ctrl ?? false,
    altKey: mods.alt ?? false,
    shiftKey: mods.shift ?? false,
    metaKey: mods.meta ?? false,
  } as KeyboardEvent;
}

function makeEngine(overrides?: Record<string, string>): ShortcutEngine {
  return createShortcutEngine({
    registry: commandRegistry,
    isMacOverride: false,
    overrides,
    buildContext(): CommandContext {
      return {
        sessions: {
          getUiState: () => ({ showSessionPanel: true }),
          setUiState: async () => {},
        } as never,
        clipboard: { writeText: async () => {}, readText: async () => '' },
        notice: () => {},
        activeTabKind: 'folder',
        activeTabId: 'tab-x',
      };
    },
  });
}

describe('shell/shortcuts 键位引擎', () => {
  it('parseBinding：解析修饰符与主键', () => {
    expect(parseBinding('Ctrl+Alt+C')).toEqual({ ctrl: true, alt: true, shift: false, key: 'c' });
    expect(parseBinding('Shift+F5')).toEqual({ ctrl: false, alt: false, shift: true, key: 'f5' });
    expect(parseBinding('Space')).toEqual({ ctrl: false, alt: false, shift: false, key: 'space' });
    expect(parseBinding('Del')).toEqual({ ctrl: false, alt: false, shift: false, key: 'delete' });
    expect(parseBinding('')).toBeNull();
    expect(parseBinding('Ctrl+')).toBeNull();
    expect(parseBinding('Ctrl+Fn')).toBeNull(); // 未知主键
    expect(parseBinding('Hyper+K')).toBeNull(); // 未知修饰符
  });

  it('formatBinding：规范化输出', () => {
    expect(formatBinding({ ctrl: true, alt: true, shift: false, key: 'c' })).toBe('Ctrl+Alt+C');
    expect(formatBinding({ ctrl: false, alt: false, shift: false, key: 'space' })).toBe('space');
    expect(formatBinding({ ctrl: false, alt: false, shift: false, key: 'a' })).toBe('A');
  });

  it('eventMatches：修饰符与主键匹配（Ctrl 别名 Cmd）', () => {
    const b = parseBinding('Ctrl+Alt+C')!;
    const notMac = false as const;
    expect(eventMatches(fakeEvent('c', { ctrl: true, alt: true }), b, notMac)).toBe(true);
    expect(eventMatches(fakeEvent('C', { ctrl: true, alt: true }), b, notMac)).toBe(true); // 大写主键
    expect(eventMatches(fakeEvent('c', { ctrl: true }), b, notMac)).toBe(false);
    expect(eventMatches(fakeEvent('c', { alt: true }), b, notMac)).toBe(false);
    expect(eventMatches(fakeEvent('v', { ctrl: true, alt: true }), b, notMac)).toBe(false);
    expect(eventMatches(fakeEvent('c', { meta: true, alt: true }), b, true)).toBe(true); // mac = Cmd
    const space = parseBinding('Space')!;
    expect(eventMatches(fakeEvent(' '), space, notMac)).toBe(true);
  });

  it('findConflicts：同键位多命令 → 冲突组', () => {
    const conflicts = findConflicts({
      a: 'Ctrl+K',
      b: 'Ctrl+K',
      c: 'Ctrl+K',
      d: 'Ctrl+L',
    });
    expect(conflicts).toHaveLength(1);
    expect(conflicts[0].binding).toBe('Ctrl+K');
    expect(conflicts[0].commands).toEqual(['a', 'b', 'c']);
    expect(findConflicts({ x: 'Ctrl+K', y: 'Ctrl+L' })).toEqual([]);
  });

  it('默认键位表（附录 G）无冲突且包含关键键位', () => {
    expect(defaultBindingConflicts()).toEqual([]);
    expect(DEFAULT_BINDINGS['folder.copyToLeft']).toBe('Ctrl+L');
    expect(DEFAULT_BINDINGS['folder.copyToRight']).toBe('Ctrl+R');
    expect(DEFAULT_BINDINGS['app.clipboardCompare']).toBe('Ctrl+Alt+C');
    expect(DEFAULT_BINDINGS['folder.markSelection']).toBe('Space');
    // 注册命令的默认键位与全局表一致（冲突自检）
    expect(commandRegistry.defaultKeyConflicts()).toEqual([]);
  });

  it('commandRegistry：内置命令注册齐全且分类可列', () => {
    const ids = commandRegistry.list().map((c) => c.id);
    for (const required of [
      'session.save', 'session.saveAs', 'session.lock',
      'app.newTab', 'app.newWindow', 'app.closeTab',
      'window.mergeAll', 'window.detachTab', 'app.settings',
      'app.showSessionPanel', 'app.clipboardCompare',
    ]) {
      expect(ids).toContain(required);
    }
    // R3-17：复制到对侧/空格标记不再是占位桩（经 folderViewBus 调视图内真实操作）
    for (const id of ['folder.copyToLeft', 'folder.copyToRight', 'folder.markSelection']) {
      expect(commandRegistry.get(id)!.phaseNote).toBeUndefined();
    }
  });

  it('ShortcutEngine.resolveCommand：按键 → 命令 id', () => {
    const engine = makeEngine();
    expect(engine.resolveCommand(fakeEvent('s', { ctrl: true }))).toBe('session.save');
    expect(engine.resolveCommand(fakeEvent('l', { ctrl: true }))).toBe('folder.copyToLeft');
    expect(engine.resolveCommand(fakeEvent(' ', {}))).toBe('folder.markSelection');
    expect(engine.resolveCommand(fakeEvent('x', {}))).toBeNull();
  });

  it('方向键：DOM 的 ArrowDown/ArrowUp 必须命中 Down/Up 绑定（FR-DIR-071 键位）', () => {
    const engine = makeEngine();
    expect(engine.resolveCommand(fakeEvent('ArrowDown', { ctrl: true, alt: true }))).toBe(
      'search.nextDiffFile',
    );
    expect(engine.resolveCommand(fakeEvent('ArrowUp', { ctrl: true, alt: true }))).toBe(
      'search.prevDiffFile',
    );
    expect(eventMatches(fakeEvent('ArrowLeft', {}), parseBinding('Left')!)).toBe(true);
    expect(
      eventMatches(fakeEvent('ArrowRight', { shift: true }), parseBinding('Shift+Right')!),
    ).toBe(true);
    // 绑定串写全名同样可用（用户自定义键位时两种写法都要认）
    expect(parseBinding('Ctrl+Alt+ArrowDown')).toEqual(parseBinding('Ctrl+Alt+Down'));
    // 修饰符不符仍不得命中
    expect(engine.resolveCommand(fakeEvent('ArrowDown', { ctrl: true }))).toBeNull();
  });

  it('rebind：无冲突生效；冲突经确认后生效、拒绝则不变', () => {
    const engine = makeEngine();
    expect(engine.rebind('session.save', 'Ctrl+Alt+P')).toBe(true);
    expect(engine.getOverrides()['session.save']).toBe('Ctrl+Alt+P');
    // 无覆盖时把 session.lock 绑到 Ctrl+S → 与默认的 session.save 冲突 → 默认拒绝
    const enginePlain = makeEngine();
    expect(enginePlain.rebind('session.lock', 'Ctrl+S')).toBe(false);
    expect(enginePlain.getOverrides()['session.lock']).toBeUndefined();
    // 冲突确认回调放行 → 覆盖生效
    const engine2 = new ShortcutEngine({
      registry: commandRegistry,
      isMacOverride: false,
      buildContext: () => ({}) as CommandContext,
      onConflict: () => true,
    });
    expect(engine2.rebind('session.lock', 'Ctrl+S')).toBe(true);
    expect(engine2.getOverrides()['session.lock']).toBe('Ctrl+S');
    expect(engine2.rebind('session.lock', 'bogus-key')).toBe(false);
  });

  it('runCommand：when 门控（folder 命令仅在目录视图可用）', () => {
    const runLog: string[] = [];
    const dispose = registerFolderView({
      copyToLeft: () => runLog.push('copyToLeft'),
      copyToRight: () => runLog.push('copyToRight'),
      markSelection: () => runLog.push('markSelection'),
    });
    try {
      const engine = makeEngine();
      engine.runCommand('folder.copyToLeft'); // activeTabKind = 'folder' → 真实操作
      expect(runLog).toEqual(['copyToLeft']);
      const homeCtx = createShortcutEngine({
        registry: commandRegistry,
        buildContext: () =>
          ({
            sessions: {} as never,
            clipboard: {} as never,
            notice: () => {},
            activeTabKind: 'home',
            activeTabId: null,
          }) as CommandContext,
      });
      // home 视图下 folder.* 的 when 不满足 → 不执行
      homeCtx.runCommand('folder.copyToLeft');
      expect(runLog).toEqual(['copyToLeft']);
    } finally {
      dispose();
    }
  });
});
