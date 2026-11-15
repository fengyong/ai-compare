import { useState } from 'react';
import type { Tab } from './store';
import { activeWindowSelector, useShell } from './store';
import { ContextMenu, type ContextMenuItem } from './ContextMenu';
import { S } from '../strings';

/**
 * 顶部标签栏（FR-WIN-001）：多会话标签 + 主页固定首标签。
 * P4 增强：右键菜单（关闭/关闭其他/固定/移动到新窗口）；
 * 标签可拖拽重排；拖出标签栏落点 → 拖出成新窗口（浏览器内窗口帧，原生窗口属 P13）。
 */
export function TabBar() {
  const win = useShell(activeWindowSelector);
  const activate = useShell((st) => st.activate);
  const closeTab = useShell((st) => st.closeTab);
  const closeOthers = useShell((st) => st.closeOthers);
  const togglePin = useShell((st) => st.togglePin);
  const detachTab = useShell((st) => st.detachTab);
  const reorderTab = useShell((st) => st.reorderTab);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
    tabId: string;
  } | null>(null);
  const [dragTabId, setDragTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  if (!win) return null;

  const openMenu = (tab: Tab, x: number, y: number) => {
    setMenu({
      x,
      y,
      tabId: tab.id,
      items: [
        {
          label: S.app.closeTab,
          onSelect: () => void closeTab(tab.id),
          disabled: tab.kind === 'home',
        },
        {
          label: S.app.closeOthers,
          onSelect: () => void closeOthers(tab.id),
          disabled: tab.kind === 'home',
        },
        {
          label: tab.pinned ? S.app.unpinTab : S.app.pinTab,
          onSelect: () => togglePin(tab.id),
          disabled: tab.kind === 'home',
        },
        { separator: true, label: '', onSelect: () => {} },
        {
          label: S.app.detachTab,
          onSelect: () => detachTab(tab.id),
          disabled: tab.kind === 'home',
        },
      ],
    });
  };

  return (
    <div
      className="aic-tabbar"
      role="tablist"
      onDragOver={(e) => {
        if (!dragTabId) return;
        e.preventDefault();
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        // 拖出标签栏右边界 → 视为拖出新窗口（在标签栏内为重排）
        setDropIndex(e.clientX > rect.right - 24 ? -1 : indexAt(e.clientX));
      }}
      onDrop={(e) => {
        if (!dragTabId) return;
        e.preventDefault();
        e.stopPropagation();
        if (dropIndex !== null && dropIndex >= 0) reorderTab(dragTabId, dropIndex);
        if (dropIndex !== null && dropIndex === -1) detachTab(dragTabId);
        setDragTabId(null);
        setDropIndex(null);
      }}
    >
      {win.tabs.map((tab: Tab, i) => (
        <div
          key={tab.id}
          role="tab"
          aria-selected={tab.id === win.activeTabId}
          className={`aic-tab${tab.id === win.activeTabId ? ' active' : ''}${dropIndex === i ? ' drop-target' : ''}`}
          onClick={() => activate(tab.id)}
          onContextMenu={(e) => {
            e.preventDefault();
            openMenu(tab, e.clientX, e.clientY);
          }}
          draggable={tab.kind !== 'home'}
          onDragStart={(e) => {
            setDragTabId(tab.id);
            // 专用 MIME：与主页文件拖放（text/plain）区分；AppShell 根 drop 据
            // 此判定"标签拖出成新窗口"（FR-WIN-001）
            e.dataTransfer.setData('application/x-aic-tab', tab.id);
            e.dataTransfer.effectAllowed = 'move';
          }}
          onDragEnd={(e) => {
            // 拖出标签栏（dropTarget 为空且不在标签栏内）→ 拖出成新窗口
            if (
              dragTabId === tab.id &&
              e.dataTransfer.dropEffect !== 'move' &&
              dropIndex === null
            ) {
              const rect = (e.currentTarget as HTMLElement)
                .closest('.aic-tabbar')
                ?.getBoundingClientRect();
              if (
                !rect ||
                e.clientX < rect.left ||
                e.clientX > rect.right ||
                e.clientY < rect.top ||
                e.clientY > rect.bottom
              ) {
                detachTab(tab.id);
              }
            }
            setDragTabId(null);
            setDropIndex(null);
          }}
          onDragOver={(e) => {
            if (dragTabId && dragTabId !== tab.id) {
              e.preventDefault();
              setDropIndex(i);
            }
          }}
          title={tab.title}
        >
          {tab.pinned && <span className="pin">📌</span>}
          <span>{tab.title}</span>
          {tab.sessionLink && (
            <span className="link-dot" title={`命名会话「${tab.sessionLink.name}」`} />
          )}
          {(tab.dirty || tab.contentDirty?.left || tab.contentDirty?.right) && (
            <span
              className="dirty-dot"
              title={
                tab.contentDirty?.left || tab.contentDirty?.right
                  ? S.session.closeGuardContentTitle
                  : S.session.closeGuardTitle
              }
            />
          )}
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
        onClick={() => {
          const home = win.tabs.find((t) => t.kind === 'home');
          if (home) activate(home.id);
        }}
      >
        {S.app.newTab}
      </button>

      {menu && (
        <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />
      )}
    </div>
  );
}

/** 按鼠标横坐标估算落点索引（取当前标签栏 DOM 位置） */
function indexAt(clientX: number): number {
  const nodes = document.querySelectorAll('.aic-tab');
  let idx = nodes.length;
  nodes.forEach((node, i) => {
    const rect = node.getBoundingClientRect();
    if (clientX < rect.left + rect.width / 2 && idx === nodes.length) idx = i;
  });
  return idx;
}
