import type { CommandCategory, CommandContext, CommandDef } from './commands';

/**
 * 菜单栏 / 工具栏模型（docs/01 附录 F 命令矩阵 + FR-UI-003）。
 * 单一数据源 = commandRegistry.list()；菜单按 category 分组、按 when 门控
 * 灰显，工具栏按收藏 id 顺序取可用命令，布局可循环切换。
 */

export type MenuId = 'session' | 'search' | 'view' | 'window' | 'tools' | 'help';

export const MENU_IDS: readonly MenuId[] = ['session', 'search', 'view', 'window', 'tools', 'help'];

export const MENU_CATEGORIES: Record<MenuId, readonly CommandCategory[]> = {
  session: ['会话'],
  search: ['搜索'],
  view: ['视图'],
  window: ['窗口'],
  tools: ['工具'],
  help: [],
};

export interface MenuEntry {
  def: CommandDef;
  enabled: boolean;
}

export interface MenuGroup {
  id: MenuId;
  entries: MenuEntry[];
}

/** when 门控：无 when 视为全局可用；不可用时灰显而非隐藏（BC 菜单惯例） */
export function commandEnabled(def: CommandDef, ctx: CommandContext): boolean {
  return def.when ? def.when(ctx) : true;
}

export function groupCommands(defs: CommandDef[], ctx: CommandContext): MenuGroup[] {
  return MENU_IDS.map((id) => {
    const categories = MENU_CATEGORIES[id];
    const entries = defs
      .filter((d) => categories.includes(d.category))
      .map((def) => ({ def, enabled: commandEnabled(def, ctx) }));
    return { id, entries };
  });
}

/** 工具栏收藏命令（按 BC 常用度排序；未注册或当前视图不可用则跳过） */
export const TOOLBAR_FAVORITES: readonly string[] = [
  'app.newTab',
  'session.save',
  'app.settings',
  'app.clipboardCompare',
  'app.clipboardHistory',
  'app.report',
  'text.toggleWrap',
  'text.toggleBookmark',
  'app.showSessionPanel',
];

export function toolbarCommands(
  defs: CommandDef[],
  ctx: CommandContext,
  favorites: readonly string[] = TOOLBAR_FAVORITES,
): CommandDef[] {
  const byId = new Map(defs.map((d) => [d.id, d]));
  return favorites
    .map((id) => byId.get(id))
    .filter((d): d is CommandDef => d !== undefined && commandEnabled(d, ctx));
}

export type ToolbarLayout = 'large' | 'compact' | 'hidden';

export const TOOLBAR_LAYOUTS: readonly ToolbarLayout[] = ['large', 'compact', 'hidden'];

/** 右键工具栏循环切换布局（FR-UI-003 工具栏显示/隐藏与布局切换） */
export function nextToolbarLayout(current: ToolbarLayout): ToolbarLayout {
  const idx = TOOLBAR_LAYOUTS.indexOf(current);
  return TOOLBAR_LAYOUTS[(idx + 1) % TOOLBAR_LAYOUTS.length];
}

/** compact 布局用短标签（取标题前两字），large 用完整标题 */
export function toolbarLabel(def: CommandDef, layout: ToolbarLayout): string {
  return layout === 'compact' ? def.title.slice(0, 2) : def.title;
}

/**
 * 统一命令执行入口：命令 run 可能是 async 且抛错，菜单/工具栏若直接
 * `void def.run(ctx)` 会产生未捕获的 Promise rejection（静默失败）。
 */
export async function runCommandSafely(
  def: CommandDef,
  ctx: CommandContext,
  notice: (msg: string) => void,
): Promise<void> {
  try {
    await def.run(ctx);
  } catch (err) {
    notice(`命令执行失败：${err instanceof Error ? err.message : String(err)}`);
  }
}
