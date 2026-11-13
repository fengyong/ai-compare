import { useCallback, useEffect, useState } from 'react';
import { sessionService } from '../../session/service';
import type { SessionNode } from '../../session/model';
import { KIND_META, type SessionKind } from '../../session/model';
import { ContextMenu, type ContextMenuItem } from '../../shell/ContextMenu';
import { useShell } from '../../shell/store';
import { S } from '../../strings';

/**
 * 会话树/管理面板（FR-HOME-002）：
 * - 保存的会话：按文件夹分组（folders 树）；右键：打开/重命名/删除/锁定；
 * - 最近会话：state.json MRU sessions（SES-001 自动保存最近会话，可返回）；
 * - 新建分支：列出各会话类型；右键「编辑」修改该类型新会话的默认设置。
 * 树数据经 SessionService.subscribe 增量刷新（保存/重命名/删除后即时更新）。
 */

interface MenuState {
  x: number;
  y: number;
  items: ContextMenuItem[];
}

export function SessionTreePanel() {
  const [tree, setTree] = useState(() => sessionService.getTree());
  const [mruSessions, setMruSessions] = useState<string[]>(() => sessionService.getMruSessions());
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [menu, setMenu] = useState<MenuState | null>(null);
  const openSavedSession = useShell((st) => st.openSavedSession);
  const openSettings = useShell((st) => st.openSettings);

  useEffect(() => {
    const refresh = () => {
      setTree(sessionService.getTree());
      setMruSessions(sessionService.getMruSessions());
    };
    refresh();
    return sessionService.subscribe(refresh);
  }, []);

  const toggle = useCallback((id: string) => {
    setCollapsed((prev) => ({ ...prev, [id]: !prev[id] }));
  }, []);

  const sessionMenu = (node: SessionNode, x: number, y: number) => {
    setMenu({
      x,
      y,
      items: [
        { label: S.tree.open, onSelect: () => void openSavedSession(node.id) },
        {
          label: node.locked ? S.tree.unlock : S.tree.lock,
          onSelect: () => void sessionService.setLocked(node.id, !node.locked),
        },
        { label: S.tree.rename, onSelect: () => void useShell.getState().renameSessionPrompt(node.id) },
        { separator: true, label: '', onSelect: () => {} },
        { label: S.tree.delete, onSelect: () => void sessionService.deleteSession(node.id) },
      ],
    });
  };

  const renderNode = (id: string, depth: number): React.ReactNode => {
    const folder = tree.folders.get(id);
    if (folder) {
      const isCollapsed = collapsed[id] === true;
      return (
        <div key={id}>
          <div
            className="aic-tree-row folder"
            style={{ paddingLeft: 8 + depth * 16 }}
            onClick={() => toggle(id)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({
                x: e.clientX,
                y: e.clientY,
                items: [{ label: S.tree.delete, onSelect: () => void sessionService.deleteFolder(id) }],
              });
            }}
          >
            <span className="twist">{isCollapsed ? '▸' : '▾'}</span>
            📂 <span>{folder.name}</span>
          </div>
          {!isCollapsed && folder.children.map((c) => renderNode(c, depth + 1))}
        </div>
      );
    }
    const node = tree.sessions.get(id);
    if (!node) return null;
    return (
      <div
        key={id}
        className="aic-tree-row session"
        style={{ paddingLeft: 8 + depth * 16 }}
        title={`${KIND_META[node.kind].title} · ${node.specs.left} ⇄ ${node.specs.right}`}
        onDoubleClick={() => void openSavedSession(node.id)}
        onContextMenu={(e) => {
          e.preventDefault();
          sessionMenu(node, e.clientX, e.clientY);
        }}
      >
        <span className="twist" />
        {KIND_META[node.kind].icon} <span>{node.name}</span>
        {node.locked && <span className="lock">🔒</span>}
      </div>
    );
  };

  return (
    <div className="aic-tree-panel">
      <div className="aic-tree-head">{S.tree.savedTitle}</div>
      {tree.rootFolderIds.length === 0 && [...tree.sessions.values()].every((s) => s.folderId !== null) && (
        <div className="aic-tree-empty">{S.tree.empty}</div>
      )}
      {tree.rootFolderIds.map((id) => renderNode(id, 0))}
      {[...tree.sessions.values()]
        .filter((s) => !s.folderId && !tree.rootFolderIds.includes(s.id))
        .map((s) => renderNode(s.id, 0))}

      <div className="aic-tree-head">{S.tree.recentTitle}</div>
      {mruSessions.length === 0 && <div className="aic-tree-empty">{S.tree.recentEmpty}</div>}
      {mruSessions.map((id) => {
        const node = tree.sessions.get(id);
        if (!node) return null;
        return (
          <div
            key={id}
            className="aic-tree-row session"
            title={`${KIND_META[node.kind].title} · ${node.specs.left} ⇄ ${node.specs.right}`}
            onDoubleClick={() => void openSavedSession(id)}
            onContextMenu={(e) => {
              e.preventDefault();
              sessionMenu(node, e.clientX, e.clientY);
            }}
          >
            <span className="twist" />
            {KIND_META[node.kind].icon} <span>{node.name}</span>
          </div>
        );
      })}

      <div className="aic-tree-head">{S.tree.newTitle}</div>
      {(Object.keys(KIND_META) as SessionKind[]).map((kind) => (
        <div
          key={kind}
          className="aic-tree-row session"
          title={KIND_META[kind].implemented ? undefined : S.tree.newDisabledPrefix + KIND_META[kind].phase}
          onContextMenu={(e) => {
            e.preventDefault();
            setMenu({
              x: e.clientX,
              y: e.clientY,
              items: [
                {
                  label: S.tree.editDefaults,
                  onSelect: () => openSettings({ kind, page: 'compare' }),
                },
              ],
            });
          }}
        >
          <span className="twist" />
          {KIND_META[kind].icon} <span>{KIND_META[kind].title}</span>
          {!KIND_META[kind].implemented && <span className="phase">{KIND_META[kind].phase}</span>}
        </div>
      ))}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
    </div>
  );
}
