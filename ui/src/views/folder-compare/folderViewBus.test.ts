import { describe, it, expect, beforeEach } from 'vitest';
import {
  activeFolderView,
  registerFolderView,
  resetFolderViewBusForTest,
  type FolderViewActions,
} from './folderViewBus';
import { commandRegistry } from '../../shell/commands';
import { createShortcutEngine } from '../../shell/shortcutEngine';
import type { CommandContext } from '../../shell/commands';

function actions(log: string[], tag: string): FolderViewActions {
  return {
    copyToLeft: () => log.push(`${tag}:left`),
    copyToRight: () => log.push(`${tag}:right`),
    markSelection: () => log.push(`${tag}:mark`),
  };
}

function keyEvent(key: string, ctrl = false): KeyboardEvent {
  return { key, ctrlKey: ctrl, altKey: false, shiftKey: false, metaKey: false } as KeyboardEvent;
}

function folderEngine() {
  return createShortcutEngine({
    registry: commandRegistry,
    isMacOverride: false,
    buildContext: () =>
      ({
        sessions: {} as never,
        clipboard: {} as never,
        notice: () => {},
        activeTabKind: 'folder',
        activeTabId: 'tab-dir',
      }) as CommandContext,
  });
}

describe('folderViewBus（R3-17：Ctrl+L/Ctrl+R/Space 从占位桩接到真实操作）', () => {
  beforeEach(() => resetFolderViewBusForTest());

  it('未注册视图时为 null，命令不抛错也不误报阶段提示', () => {
    expect(activeFolderView()).toBeNull();
    const notices: string[] = [];
    const engine = createShortcutEngine({
      registry: commandRegistry,
      isMacOverride: false,
      buildContext: () =>
        ({
          sessions: {} as never,
          clipboard: {} as never,
          notice: (m: string) => notices.push(m),
          activeTabKind: 'folder',
          activeTabId: 'tab-dir',
        }) as CommandContext,
    });
    engine.runCommand('folder.copyToLeft');
    engine.runCommand('folder.copyToRight');
    engine.runCommand('folder.markSelection');
    expect(notices).toEqual([]);
  });

  it('栈语义：后挂载的视图优先接命令，卸载只摘除自己的条目', () => {
    const log: string[] = [];
    const disposeA = registerFolderView(actions(log, 'A'));
    const disposeB = registerFolderView(actions(log, 'B'));
    expect(activeFolderView()).not.toBeNull();
    activeFolderView()!.copyToLeft();
    expect(log).toEqual(['B:left']);
    // B 卸载 → A 重新接命令（不会被一并清空）
    disposeB();
    activeFolderView()!.copyToRight();
    expect(log).toEqual(['B:left', 'A:right']);
    disposeA();
    expect(activeFolderView()).toBeNull();
  });

  it('三个键位各自派发到对应动作，且每次只触发一次', () => {
    const log: string[] = [];
    const dispose = registerFolderView(actions(log, 'V'));
    try {
      const engine = folderEngine();
      expect(engine.resolveCommand(keyEvent('l', true))).toBe('folder.copyToLeft');
      expect(engine.resolveCommand(keyEvent('r', true))).toBe('folder.copyToRight');
      expect(engine.resolveCommand(keyEvent(' '))).toBe('folder.markSelection');
      engine.runCommand('folder.copyToLeft');
      engine.runCommand('folder.copyToRight');
      engine.runCommand('folder.markSelection');
      // Space 此前由视图 window 捕获监听 + 桩命令双路径处理；现只剩命令一条路径，
      // 一次按键不得触发两次标记（两次 toggle 会互相抵消）
      expect(log).toEqual(['V:left', 'V:right', 'V:mark']);
    } finally {
      dispose();
    }
  });

  it('命令定义不再带交付阶段占位说明', () => {
    for (const id of ['folder.copyToLeft', 'folder.copyToRight', 'folder.markSelection']) {
      const def = commandRegistry.get(id)!;
      expect(def.phaseNote).toBeUndefined();
      expect(def.defaultKeys).toBeTruthy();
    }
  });
});
