import { create } from 'zustand';
import type { DirRow } from '../protocol/types';
import { basename, joinPath, looksTextual } from '../lib/env';
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

/** 文本子会话的文件引用（UI 侧视图模型；协议侧为纯路径字符串） */
export interface FileRef {
  path: string;
  name: string;
}

export interface FolderTabData {
  left: string;
  right: string;
  /** 客户端分配的引擎会话句柄（生成协议 DirOpenParams.sessionId），标签创建时即确定 */
  sessionId: string;
}

export interface TextTabData {
  sessionId: string;
  left: FileRef;
  right: FileRef;
  /** 父目录标签（差异回传链路 docs/03 §5.1 的 UI 侧关联；引擎侧写回属 P1） */
  parentTabId?: string;
  parentFolderSessionId?: string;
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
    const tab: Tab = {
      id,
      kind: 'folder',
      title,
      folder: { left, right, sessionId: engine.newSessionId('dir') },
    };
    set((st) => ({ tabs: [...st.tabs, tab], activeTabId: id, notice: null }));
  },

  /**
   * 双击目录行 → 打开文本子会话。
   *
   * P0 说明：引擎侧 dir/openChild（FormatRegistry 探测，docs/03 §3.5）与
   * 父会话差异回传（§5.1）均未开放（P1/P2）；UI 以行 id（相对路径）拼出
   * 两侧完整路径直接 text/open，视图类型按扩展名近似判定。
   */
  async openTextChild(parentTabId, row) {
    const parent = get().tabs.find((t) => t.id === parentTabId);
    const folder = parent?.folder;
    if (!folder) return;
    if (!looksTextual(row.name)) {
      get().setNotice(S.text.unsupportedView);
      return;
    }
    const left = joinPath(folder.left, row.id);
    const right = joinPath(folder.right, row.id);
    try {
      const sessionId = engine.newSessionId('text');
      await engine.openText({ sessionId, left, right, options: null });
      const id = nextTabId('text');
      const tab: Tab = {
        id,
        kind: 'text',
        title: `${row.name}（${S.tab.textTitleSuffix}）`,
        text: {
          sessionId,
          left: { path: left, name: row.name },
          right: { path: right, name: row.name },
          parentTabId,
          parentFolderSessionId: folder.sessionId,
        },
      };
      set((st) => ({ tabs: [...st.tabs, tab], activeTabId: id, notice: null }));
    } catch (err) {
      get().setNotice(`${S.folder.openFail}: ${err instanceof Error ? err.message : String(err)}`);
    }
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

    // session/close 幂等（引擎侧不存在也成功）；目录会话额外释放 UI 缓存
    if (tab.text?.sessionId) {
      try {
        await engine.closeSession({ sessionId: tab.text.sessionId });
      } catch (err) {
        console.warn('[session/close] 失败', err);
      }
    }
    if (tab.folder?.sessionId) {
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
