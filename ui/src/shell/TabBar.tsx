import type { Tab } from './store';
import { useShell } from './store';
import { S } from '../strings';

/** 顶部标签栏（FR-WIN-001 的最小实现：多会话标签 + 主页固定首标签） */
export function TabBar() {
  const tabs = useShell((st) => st.tabs);
  const activeTabId = useShell((st) => st.activeTabId);
  const activate = useShell((st) => st.activate);
  const closeTab = useShell((st) => st.closeTab);

  return (
    <div className="aic-tabbar" role="tablist">
      {tabs.map((tab: Tab) => (
        <div
          key={tab.id}
          role="tab"
          aria-selected={tab.id === activeTabId}
          className={`aic-tab${tab.id === activeTabId ? ' active' : ''}`}
          onClick={() => activate(tab.id)}
          title={tab.title}
        >
          <span>{tab.title}</span>
          {tab.kind !== 'home' && (
            <button
              className="close"
              aria-label={S.app.closeTab}
              title={S.app.closeTab}
              onClick={(e) => {
                e.stopPropagation();
                void closeTab(tab.id);
              }}
            >
              ×
            </button>
          )}
        </div>
      ))}
      <button
        className="aic-tabbar-add"
        title={S.app.homeTab}
        aria-label={S.app.homeTab}
        onClick={() => activate('home')}
      >
        {S.app.newTab}
      </button>
    </div>
  );
}
