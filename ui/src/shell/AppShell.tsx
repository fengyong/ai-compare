import { useEffect, useState } from 'react';
import { useShell } from './store';
import { TabBar } from './TabBar';
import { HomeView } from '../views/home/HomeView';
import { FolderCompareView } from '../views/folder-compare/FolderCompareView';
import { TextCompareView } from '../views/text-compare/TextCompareView';
import { applyTheme } from '../theme/palette';
import {
  getEngineState,
  retryEngine,
  subscribeEngineState,
  switchToMockEngine,
} from '../ipc/engineProcess';
import { S } from '../strings';

/**
 * 应用壳（docs/03 §3.4.1 M30 的 P0 子集）：顶部标签栏 + 视图路由 + 引擎状态面。
 * 窗口/菜单/命令面板/设置界面等属 P4 阶段交付。
 */
export function AppShell() {
  const tabs = useShell((st) => st.tabs);
  const activeTabId = useShell((st) => st.activeTabId);
  const notice = useShell((st) => st.notice);
  const setNotice = useShell((st) => st.setNotice);
  const [engineState, setEngineState] = useState(getEngineState);

  useEffect(() => {
    applyTheme();
    return subscribeEngineState(() => setEngineState(getEngineState()));
  }, []);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? tabs[0];

  return (
    <div className="aic-app">
      <TabBar />

      {engineState.status === 'degraded' && (
        <div className="aic-banner" role="alert">
          <b>{S.engine.bannerTitle}</b>
          <span className="msg">{engineState.message || S.engine.missingHint}</span>
          <button onClick={() => retryEngine()}>{S.engine.retry}</button>
          <button onClick={() => switchToMockEngine()}>{S.engine.useMock}</button>
        </div>
      )}
      {engineState.status === 'mock' && (
        <div className="aic-banner info">
          <span className="msg">{S.engine.bannerInfo}</span>
        </div>
      )}

      <div className="aic-view-host">
        {activeTab?.kind === 'home' && <HomeView />}
        {activeTab?.kind === 'folder' && activeTab.folder && (
          <FolderCompareView key={activeTab.id} tab={activeTab} />
        )}
        {activeTab?.kind === 'text' && activeTab.text && (
          <TextCompareView key={activeTab.id} tab={activeTab} />
        )}
      </div>

      <div className="aic-statusbar">
        <span>
          <span className={`aic-engine-dot ${engineState.status}`} />
          {engineState.message}
        </span>
        <span className="spacer" />
        {notice && (
          <span style={{ color: 'var(--aic-danger)' }} onClick={() => setNotice(null)}>
            {notice}
          </span>
        )}
        <span>{activeTab?.title}</span>
      </div>
    </div>
  );
}
