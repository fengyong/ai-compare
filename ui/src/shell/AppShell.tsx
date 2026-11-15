import { useEffect, useState } from 'react';
import { activeWindowSelector, useShell } from './store';
import { TabBar } from './TabBar';
import { MenuBar } from './MenuBar';
import { ToolBar } from './ToolBar';
import { CloseGuardDialog, ConfirmDialog, PromptDialog, SaveAsDialog } from './Dialogs';
import { SettingsDialog } from './settings/SettingsDialog';
import { HomeView } from '../views/home/HomeView';
import { FolderCompareView } from '../views/folder-compare/FolderCompareView';
import { FolderSyncView } from '../views/folder-sync/FolderSyncView';
import { FolderMergeView } from '../views/folder-merge/FolderMergeView';
import { PictureCompareView } from '../views/picture-compare/PictureCompareView';
import { TableCompareView } from '../views/table-compare/TableCompareView';
import { HexCompareView } from '../views/hex-compare/HexCompareView';
import { Merge3View } from '../views/merge3/Merge3View';
import { ClipboardHistoryPanel } from '../views/home/ClipboardHistoryPanel';
import { ReportDialog } from '../views/report/ReportDialog';
import type { Tab } from './store';
import { TextCompareView } from '../views/text-compare/TextCompareView';
import { TextEditView } from '../views/text-edit/TextEditView';
import { applyTheme } from '../theme/palette';
import { sessionService } from '../session/service';
import {
  getEngineState,
  retryEngine,
  subscribeEngineState,
  subscribeResync,
  switchToMockEngine,
} from '../ipc/engineProcess';
import { setNoticeSink, createViewCtx } from './viewRegistry';
import { attachNativeExitGuard } from './nativeExit';
import { commandRegistry } from './commands';
import { createShortcutEngine, type ShortcutEngine } from './shortcutEngine';
import { S } from '../strings';

/**
 * 应用壳（docs/03 §3.4.1 M30）：
 * - 顶部标签栏（多窗口帧由窗口切换条呈现，FR-WIN-002）；
 * - 引擎横幅状态机（正常/重连中/恢复会话/失败/演示模式）；
 * - 命令 + 快捷键引擎挂载；会话服务启动（sessions/state.json 加载与恢复提示）；
 * - 对话框宿主（设置/关闭确认/另存/重命名）。
 */

let shortcutEngine: ShortcutEngine | null = null;

export function AppShell() {
  const win = useShell(activeWindowSelector);
  const windows = useShell((st) => st.windows);
  const activeWindowId = useShell((st) => st.activeWindowId);
  const notice = useShell((st) => st.notice);
  const setNotice = useShell((st) => st.setNotice);
  const settingsDialog = useShell((st) => st.settingsDialog);
  const activateWindow = useShell((st) => st.activateWindow);
  const mergeAllWindows = useShell((st) => st.mergeAllWindows);
  const openNewWindow = useShell((st) => st.openNewWindow);
  const rebindSession = useShell((st) => st.rebindSession);
  const [engineState, setEngineState] = useState(getEngineState);
  const [initNotice, setInitNotice] = useState<string | null>(null);

  // ---- 会话服务启动：sessions/state.json 加载 + 损坏恢复提示（SET-001/R12） ----
  useEffect(() => {
    let disposed = false;
    void sessionService.init().then((report) => {
      if (disposed) return;
      const parts: string[] = [];
      if (report.recoveredFromBackup.length > 0) {
        parts.push(`已从备份自动恢复：${report.recoveredFromBackup.join('、')}`);
      }
      for (const lost of report.lost) {
        parts.push(`设置文件损坏且备份不可用，已重置：${lost.file}`);
      }
      if (report.portable) parts.push('便携模式（设置存于程序目录）');
      if (parts.length > 0) setInitNotice(parts.join('；'));
    });
    return () => {
      disposed = true;
    };
  }, []);

  // ---- 原生窗口关闭守卫（R3-05：未落盘编辑/未保存会话先提示，处理完才退出）----
  useEffect(() => attachNativeExitGuard(), []);

  // ---- 横幅状态机订阅 + 会话恢复事件（replay 降级 → rebindSession） ----
  useEffect(() => {
    applyTheme();
    const unsubEngine = subscribeEngineState(() => setEngineState(getEngineState()));
    const unsubResync = subscribeResync((outcome) => {
      if (outcome.mode === 'replay') {
        rebindSession(outcome.oldSessionId, outcome.newSessionId);
      }
    });
    return () => {
      unsubEngine();
      unsubResync();
    };
  }, [rebindSession]);

  // ---- notice 出口 + 快捷键引擎挂载 ----
  useEffect(() => {
    setNoticeSink((msg) => useShell.getState().setNotice(msg));
    const ctx = createViewCtx(sessionService);
    shortcutEngine = createShortcutEngine({
      registry: commandRegistry,
      buildContext: () => {
        const st = useShell.getState();
        const w = st.windows.find((x) => x.id === st.activeWindowId);
        const tab = w?.tabs.find((t) => t.id === w.activeTabId);
        return {
          ...ctx,
          notice: (msg: string) => useShell.getState().setNotice(msg),
          activeTabKind: tab ? tab.kind : null,
          activeTabId: tab?.id ?? null,
        };
      },
    });
    shortcutEngine.attach();
    return () => {
      shortcutEngine?.detach();
      shortcutEngine = null;
      setNoticeSink(null);
    };
  }, []);

  if (!win) return null;

  const activeTab = win.tabs.find((t) => t.id === win.activeTabId) ?? win.tabs[0];

  return (
    <div
      className="aic-app"
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes('application/x-aic-tab')) e.preventDefault();
      }}
      onDrop={(e) => {
        // 标签拖出成新窗口：拖到应用内任何非标签栏位置（FR-WIN-001）
        const tabId = e.dataTransfer.getData('application/x-aic-tab');
        if (tabId) {
          e.preventDefault();
          useShell.getState().detachTab(tabId);
        }
      }}
    >
      <MenuBar />
      <ToolBar />
      <TabBar />

      {engineState.status === 'degraded' && (
        <div className="aic-banner" role="alert">
          <b>{engineState.phase === 'failed' ? S.engine.bannerFailed : S.engine.bannerTitle}</b>
          <span className="msg">{engineState.message || S.engine.missingHint}</span>
          <button onClick={() => retryEngine()}>{S.engine.retry}</button>
          <button onClick={() => switchToMockEngine()}>{S.engine.useMock}</button>
        </div>
      )}
      {engineState.phase === 'reconnecting' && engineState.status !== 'degraded' && (
        <div className="aic-banner info" role="status">
          <span className="msg">{S.engine.bannerReconnecting}</span>
        </div>
      )}
      {engineState.phase === 'resyncing' && (
        <div className="aic-banner info" role="status">
          <span className="msg">{S.engine.bannerResyncing}</span>
        </div>
      )}
      {engineState.status === 'mock' && (
        <div className="aic-banner info">
          <span className="msg">{S.engine.bannerInfo}</span>
        </div>
      )}
      {initNotice && (
        <div className="aic-banner warn" role="status" onClick={() => setInitNotice(null)}>
          <span className="msg">{initNotice}</span>
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
        {activeTab?.kind === 'edit' && activeTab.edit && (
          <TextEditView key={activeTab.id} tab={activeTab} />
        )}
        {activeTab?.kind === 'sync' && activeTab.sync && (
          <FolderSyncView key={activeTab.id} tab={activeTab} />
        )}
        {activeTab?.kind === 'merge' && activeTab.merge && (
          <FolderMergeView key={activeTab.id} tab={activeTab} />
        )}
        {activeTab?.kind === 'picture' && activeTab.picture && (
          <PictureCompareView
            key={activeTab.id}
            tab={
              activeTab as Tab & {
                kind: 'picture';
                picture: { left: string; right: string; sessionId: string };
              }
            }
          />
        )}
        {activeTab?.kind === 'table' && activeTab.table && (
          <TableCompareView
            key={activeTab.id}
            tab={
              activeTab as Tab & {
                kind: 'table';
                table: { left: string; right: string; sessionId: string };
              }
            }
          />
        )}
        {activeTab?.kind === 'hex' && activeTab.hex && (
          <HexCompareView
            key={activeTab.id}
            tab={
              activeTab as Tab & {
                kind: 'hex';
                hex: { left: string; right: string; sessionId: string };
              }
            }
          />
        )}
        {activeTab?.kind === 'merge3' && activeTab.merge3 && (
          <Merge3View
            key={activeTab.id}
            tab={
              activeTab as Tab & {
                kind: 'merge3';
                merge3: { base: string; left: string; right: string; sessionId: string };
              }
            }
          />
        )}
      </div>

      {/* 窗口切换条（浏览器内窗口帧；>1 窗口时出现，FR-WIN-002 循环窗口/合并入口） */}
      {windows.length > 1 && (
        <div className="aic-window-strip">
          <span className="label">{S.app.windowStrip}</span>
          {windows.map((w) => (
            <button
              key={w.id}
              className={w.id === activeWindowId ? 'active' : ''}
              onClick={() => activateWindow(w.id)}
              title={w.title}
            >
              {w.title || S.app.homeTab}
            </button>
          ))}
          <button onClick={() => openNewWindow()} title={S.app.newWindow}>
            ＋
          </button>
          <button onClick={() => void mergeAllWindows()} title={S.app.mergeAll}>
            {S.app.mergeAll}
          </button>
        </div>
      )}

      <div className="aic-statusbar">
        <span>
          <span className={`aic-engine-dot ${engineState.status}`} />
          {engineState.message}
        </span>
        <span className="spacer" />
        {activeTab && activeTab.kind !== 'home' && (
          <button
            className="aic-report-entry"
            onClick={() => useShell.getState().openReportDialog()}
          >
            {S.report.open}
          </button>
        )}
        {notice && (
          <span className="aic-notice" onClick={() => setNotice(null)}>
            {notice}
          </span>
        )}
        <span>{activeTab?.title}</span>
      </div>

      {settingsDialog !== null && (
        <SettingsDialog
          key={`${settingsDialog.page ?? ''}:${settingsDialog.kind ?? ''}:${settingsDialog.sessionTabId ?? ''}`}
          initialPage={settingsDialog.page}
          kind={settingsDialog.kind}
          sessionTabId={settingsDialog.sessionTabId}
        />
      )}
      <CloseGuardDialog />
      <SaveAsDialog />
      <PromptDialog />
      <ConfirmDialog />
      <ClipboardHistoryPanel />
      <ReportDialog />
    </div>
  );
}
