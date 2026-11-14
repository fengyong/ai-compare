import { S } from '../strings';
import { commandRegistry } from './commands';
import { buildCommandContext } from './MenuBar';
import { runCommandSafely, toolbarCommands, toolbarLabel, type ToolbarLayout } from './menuModel';
import { useShell } from './store';

/** 工具栏（FR-UI-003）：收藏命令按可用门控显示，右键循环切换 大按钮/紧凑/隐藏 */
export function ToolBar() {
  const layout: ToolbarLayout = useShell((st) => st.toolbarLayout);
  const cycle = useShell((st) => st.cycleToolbarLayout);
  const notice = useShell((st) => st.setNotice);
  useShell((st) => st.windows); // 标签变化触发命令可用性重算
  useShell((st) => st.activeWindowId); // 切换活动窗口不改 windows 引用，必须单独订阅
  if (layout === 'hidden') return null;
  const ctx = buildCommandContext();
  const defs = toolbarCommands(commandRegistry.list(), ctx);
  return (
    <div
      className={`aic-toolbar ${layout}`}
      title={S.menu.toolbarHint}
      onContextMenu={(e) => {
        e.preventDefault();
        cycle();
      }}
    >
      {defs.map((d) => (
        <button
          key={d.id}
          title={`${d.title}${d.defaultKeys ? ` (${d.defaultKeys})` : ''}`}
          onClick={() => void runCommandSafely(d, ctx, notice)}
        >
          {toolbarLabel(d, layout)}
        </button>
      ))}
    </div>
  );
}
