import { describe, expect, it } from 'vitest';
import { commandRegistry } from '../../shell/commands';

describe('队列 6：文本视图命令（FR-TXT-002 / FR-TXT-023）', () => {
  it('换行与书签命令全部注册，默认键位无冲突', () => {
    const ids = [
      'text.toggleWrap',
      'text.toggleBookmark',
      'text.nextBookmark',
      'text.prevBookmark',
      'text.clearBookmarks',
    ];
    for (const id of ids) expect(commandRegistry.get(id)).toBeDefined();
    expect(commandRegistry.get('text.toggleBookmark')?.defaultKeys).toBe('Ctrl+F2');
    expect(commandRegistry.get('text.nextBookmark')?.defaultKeys).toBe('F2');
    expect(commandRegistry.get('text.prevBookmark')?.defaultKeys).toBe('Shift+F2');
    expect(commandRegistry.defaultKeyConflicts()).toEqual([]);
  });

  it('when 门控：仅文本比较视图可用', () => {
    const when = commandRegistry.get('text.toggleBookmark')?.when;
    expect(when?.({ activeTabKind: 'text' } as never)).toBe(true);
    expect(when?.({ activeTabKind: 'folder' } as never)).toBe(false);
    expect(when?.({ activeTabKind: null } as never)).toBe(false);
  });
});

describe('R3-04：文本文件落盘命令（FR-TXT-020）', () => {
  it('保存/另存命令已注册，键位无冲突且仅文本视图可用', () => {
    const save = commandRegistry.get('text.saveFile');
    const saveAs = commandRegistry.get('text.saveFileAs');
    expect(save).toBeDefined();
    expect(saveAs).toBeDefined();
    expect(save?.defaultKeys).toBe('Ctrl+Shift+S');
    // Ctrl+S 仍属"保存会话"（不改既有键位语义）
    expect(commandRegistry.get('session.save')?.defaultKeys).toBe('Ctrl+S');
    expect(commandRegistry.defaultKeyConflicts()).toEqual([]);
    for (const def of [save, saveAs]) {
      expect(def?.when?.({ activeTabKind: 'text' } as never)).toBe(true);
      expect(def?.when?.({ activeTabKind: 'folder' } as never)).toBe(false);
    }
  });
});
