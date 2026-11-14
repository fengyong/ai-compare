import { useEffect, useState } from 'react';
import { sessionService } from '../session/service';
import { S } from '../strings';
import { commandRegistry, type CommandContext } from './commands';
import { groupCommands, type MenuId } from './menuModel';
import { useShell } from './store';
import { createViewCtx } from './viewRegistry';

/** 从当前壳状态构建命令上下文（菜单/工具栏/快捷键三处同口径） */
export function buildCommandContext(): CommandContext {
  const st = useShell.getState();
  const win = st.windows.find((w) => w.id === st.activeWindowId);
  const tab = win?.tabs.find((t) => t.id === win.activeTabId);
  return {
    ...createViewCtx(sessionService),
    activeTabKind: tab ? tab.kind : null,
    activeTabId: tab?.id ?? null,
  };
}

const MENU_LABELS: Record<MenuId, string> = {
  session: S.menu.session,
  view: S.menu.view,
  window: S.menu.window,
  tools: S.menu.tools,
  help: S.menu.help,
};

/** 菜单栏（docs/01 附录 F）：按 category 分组、when 门控灰显、显示默认键位 */
export function MenuBar() {
  const [openId, setOpenId] = useState<MenuId | null>(null);
  const notice = useShell((st) => st.setNotice);
  useShell((st) => st.windows); // 标签/窗口变化触发命令可用性重算
  useShell((st) => st.activeWindowId);
  const ctx = buildCommandContext();
  const groups = groupCommands(commandRegistry.list(), ctx);

  useEffect(() => {
    if (openId === null) return;
    const close = () => setOpenId(null);
    document.addEventListener('click', close);
    return () => document.removeEventListener('click', close);
  }, [openId]);

  const run = (id: string | undefined, enabled: boolean) => {
    setOpenId(null);
    if (!id || !enabled) return;
    const def = commandRegistry.get(id);
    if (def) void def.run(ctx);
  };

  return (
    <div className="aic-menubar">
      {groups.map((group) => (
        <div className="aic-menu" key={group.id}>
          <button
            className={`aic-menu-title${openId === group.id ? ' open' : ''}`}
            onClick={(e) => {
              e.stopPropagation();
              setOpenId((v) => (v === group.id ? null : group.id));
            }}
          >
            {MENU_LABELS[group.id]}
          </button>
          {openId === group.id && (
            <div className="aic-menu-dropdown" onClick={(e) => e.stopPropagation()}>
              {group.entries.map(({ def, enabled }) => (
                <button
                  key={def.id}
                  className="aic-menu-item"
                  disabled={!enabled}
                  title={def.phaseNote ?? def.title}
                  onClick={() => run(def.id, enabled)}
                >
                  <span>{def.title}</span>
                  {def.defaultKeys && <span className="aic-menu-keys">{def.defaultKeys}</span>}
                </button>
              ))}
              {group.id === 'help' && (
                <>
                  <button className="aic-menu-item" onClick={() => { setOpenId(null); notice(S.menu.aboutHint); }}>
                    <span>{S.menu.about}</span>
                  </button>
                  <button className="aic-menu-item" onClick={() => { setOpenId(null); notice(S.menu.docs); }}>
                    <span>{S.menu.docsLabel}</span>
                  </button>
                </>
              )}
              {group.entries.length === 0 && group.id !== 'help' && (
                <span className="aic-menu-empty">{S.menu.noCommands}</span>
              )}
            </div>
          )}
        </div>
      ))}
      <span className="aic-menubar-spacer" />
      <span className="aic-menubar-hint">{S.menu.toolbarHint}</span>
    </div>
  );
}
