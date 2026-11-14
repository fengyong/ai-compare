import { describe, expect, it } from 'vitest';
import type { CommandContext, CommandDef } from './commands';
import {
  MENU_IDS,
  TOOLBAR_FAVORITES,
  TOOLBAR_LAYOUTS,
  commandEnabled,
  groupCommands,
  nextToolbarLayout,
  runCommandSafely,
  toolbarCommands,
  toolbarLabel,
} from './menuModel';

const def = (id: string, category: CommandDef['category'], when?: CommandDef['when']): CommandDef => ({
  id,
  title: `命令-${id}`,
  category,
  when,
  run: () => undefined,
});

const ctx = (kind: CommandContext['activeTabKind'] = null): CommandContext => ({
  activeTabKind: kind,
  activeTabId: null,
  sessions: {} as never,
  clipboard: {} as never,
  notice: () => undefined,
});

describe('菜单栏分组（附录 F 命令矩阵）', () => {
  it('五个菜单按 category 分组，help 无注册命令', () => {
    const groups = groupCommands(
      [
        def('session.save', '会话'),
        def('text.toggleWrap', '视图'),
        def('app.newTab', '窗口'),
        def('app.report', '工具'),
      ],
      ctx()
    );
    expect(groups.map((g) => g.id)).toEqual(MENU_IDS);
    expect(groups[0].entries.map((e) => e.def.id)).toEqual(['session.save']);
    expect(groups[1].entries.map((e) => e.def.id)).toEqual(['text.toggleWrap']);
    expect(groups[4].entries).toHaveLength(0);
  });

  it('when 门控：不适用命令灰显而非隐藏', () => {
    const d = def('text.only', '视图', (c) => c.activeTabKind === 'text');
    expect(commandEnabled(d, ctx('text'))).toBe(true);
    expect(commandEnabled(d, ctx('folder'))).toBe(false);
    const g = groupCommands([d], ctx('folder'))[1];
    expect(g.entries).toHaveLength(1);
    expect(g.entries[0].enabled).toBe(false);
  });
});

describe('工具栏（FR-UI-003）', () => {
  it('收藏顺序过滤：未注册与不可用命令跳过', () => {
    const defs = [
      def('app.newTab', '窗口'),
      def('app.report', '工具', (c) => c.activeTabKind !== null && c.activeTabKind !== 'home'),
      def('app.other', '工具'),
    ];
    expect(toolbarCommands(defs, ctx('text'), ['app.newTab', 'app.report', 'app.other']).map((d) => d.id))
      .toEqual(['app.newTab', 'app.report', 'app.other']);
    const home = toolbarCommands(defs, ctx('home'), ['app.report', 'app.newTab', 'missing']);
    expect(home.map((d) => d.id)).toEqual(['app.newTab']);
    expect(TOOLBAR_FAVORITES.length).toBeGreaterThan(0);
  });

  it('runCommandSafely 捕获同步/异步命令异常并提示（无未处理 rejection）', async () => {
    const notices: string[] = [];
    const sync: CommandDef = {
      id: 'x1', title: '同步出错', category: '工具',
      run: () => { throw new Error('sync-boom'); },
    };
    const asyncDef: CommandDef = {
      id: 'x2', title: '异步出错', category: '工具',
      run: async () => { throw new Error('async-boom'); },
    };
    await runCommandSafely(sync, ctx(), (m) => notices.push(m));
    await runCommandSafely(asyncDef, ctx(), (m) => notices.push(m));
    expect(notices).toHaveLength(2);
    expect(notices[0]).toContain('sync-boom');
    expect(notices[1]).toContain('async-boom');
  });

  it('布局循环 large→compact→hidden→large；compact 短标签', () => {
    expect(nextToolbarLayout('large')).toBe('compact');
    expect(nextToolbarLayout('compact')).toBe('hidden');
    expect(nextToolbarLayout('hidden')).toBe('large');
    expect(TOOLBAR_LAYOUTS).toEqual(['large', 'compact', 'hidden']);
    const d = def('session.save', '会话');
    expect(toolbarLabel(d, 'large')).toBe(d.title);
    expect(toolbarLabel(d, 'compact')).toBe(d.title.slice(0, 2));
  });
});
