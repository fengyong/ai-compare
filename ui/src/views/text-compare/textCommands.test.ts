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
