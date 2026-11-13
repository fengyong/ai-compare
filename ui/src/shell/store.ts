import { create } from 'zustand';
import type { DirRow, FileRef } from '../protocol/types';
import { basename } from '../lib/env';
import { S } from '../strings';
import * as engine from '../ipc/engineProcess';
import { releaseDirCache } from '../views/folder-compare/dirRowCache';

/**
 * 应用壳状态（M30 应用壳的最小实现）：
 * 多标签（主页固定首标签 + 文件夹/文本比较会话标签）+ 激活路由 + 子会话联动。
 *
 * 不引入 Redux（任务约束）；zustand 仅承载"标签/会话"这类轻量 UI 状态，
 * 大数据（目录行缓存、文本行）留在各视图的非响应式缓存（见 dirRowCache.ts）。
 */

export interface FolderTabData {
  left: string;
  right: string;
  /** dir/open 成功后回填；用于子会话回传寻址 */
  sessionId?: string;
}

export interface TextTabData {
  sessionId: string;
  left: FileRef;
  right: FileRef;
  /** 父目录会话（差异回传链路，docs/03 §5.1） */
  parentTabId?: string;
  parentFolderSessionId?: string;
  parentRowId?: number;
}

export interface Tab {
  id: string;
  kind: 'home' | 'folder' | 'text';
  title: string;
  folder?: FolderTabData;
  text?: TextTabData;
}

export interface ShellStore {
  tabs: Tab[];
  activeTabId: string;
  /** 状态栏一次性提示（如打开失败原因） */
  notice: string | null;

  openFolderTab(left: string, right: string): void;
  openTextChild(parentTabId: string, row: DirRow): Promise<void>;
  setFolderSession(tabId: string, sessionId: string): void;
  activate(tabId: string): void;
  closeTab(tabId: string): Promise<void>;
  setNotice(msg: string | null): void;
}

const HOME_TAB: Tab = { id: 'home', kind: 'home', title: S.app.homeTab };

let tabSeq = 1;
const nextTabId = (kind: string) => `tab-${kind}-${tabSeq++}`;

export const useShell = create<ShellStore>((set, get) => ({
  tabs: [HOME_TAB],
  activeTabId: HOME_TAB.id,
  notice: null,

  openFolderTab(left, right) {
    const id = nextTabId('dir');
    const title = `${basename(left)} ⇄ ${basename(right)}`;
    const tab: Tab = { id, kind: 'folder', title, folder: { left, right } };
    set((st) => ({ tabs: [...st.tabs, tab], activeTabId: id, notice: null }));
  },

  async openTextChild(parentTabId, row) {
    const parent = get().tabs.find((t) => t.id === parentTabId);
    const folderSessionId = parent?.folder?.sessionId;
    if (!folderSessionId) return;
    try {
      // 1) 引擎解析打开方式（FormatRegistry 探测，docs/03 §3.5）
      const child = await engine.openChild({ sessionId: folderSessionId, rowId: row.rowId });
      if (child.viewKind !== 'text') {
        get().setNotice(S.text.unsupportedView);
        return;
      }
      // 2) 创建文本比较子会话（携带父会话引用 → 关闭时引擎回传 fileOverride）
      const opened = await engine.openText({
        left: child.left,
        right: child.right,
        parent: { sessionId: folderSessionId, rowId: row.rowId },
      });
      const id = nextTabId('text');
      const tab: Tab = {
        id,
        kind: 'text',
        title: `${row.name}（${S.tab.textTitleSuffix}）`,
        text: {
          sessionId: opened.sessionId,
          left: child.left,
          right: child.right,
          parentTabId,
          parentFolderSessionId: folderSessionId,
          parentRowId: row.rowId,
        },
      };
      set((st) => ({ tabs: [...st.tabs, tab], activeTabId: id, notice: null }));
    } catch (err) {
      get().setNotice(`${S.folder.openFail}: ${err instanceof Error ? err.message : String(err)}`);
    }
  },

  setFolderSession(tabId, sessionId) {
    set((st) => ({
      tabs: st.tabs.map((t) =>
        t.id === tabId && t.folder ? { ...t, folder: { ...t.folder, sessionId } } : t
      ),
    }));
  },

  activate(tabId) {
    set({ activeTabId: tabId });
  },

  setNotice(msg) {
    set({ notice: msg });
  },

  async closeTab(tabId) {
    const st0 = get();
    const tab = st0.tabs.find((t) => t.id === tabId);
    if (!tab || tab.kind === 'home') return;

    if (tab.kind === 'text' && tab.text) {
      // 差异回传：引擎把本次判定写回父目录行（dir/delta upsert 该行），
      // 目录视图的 delta 订阅收到后自然重新着色（docs/03 §5.1 时序 12~14）。
      try {
        await engine.closeText({ sessionId: tab.text.sessionId, reportStatus: true });
      } catch (err) {
        console.warn('[text/close] 回传失败', err);
      }
    }
    if (tab.kind === 'folder' && tab.folder?.sessionId) {
      try {
        await engine.closeSession({ sessionId: tab.folder.sessionId });
      } catch (err) {
        console.warn('[session/close] 失败', err);
      }
      releaseDirCache(tab.folder.sessionId);
    }

    set((st) => {
      const idx = st.tabs.findIndex((t) => t.id === tabId);
      const tabs = st.tabs.filter((t) => t.id !== tabId);
      const activeTabId =
        st.activeTabId === tabId ? tabs[Math.max(0, idx - 1)]?.id ?? HOME_TAB.id : st.activeTabId;
      return { tabs, activeTabId };
    });
  },
}));
