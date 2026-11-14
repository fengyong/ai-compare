import { useEffect, useState } from 'react';
import { sessionService } from '../../session/service';
import { KIND_META, type SessionKind } from '../../session/model';
import { viewRegistry, type ViewPlugin } from '../../shell/viewRegistry';
import { useShell } from '../../shell/store';
import { extractPathsFromDataTransfer, pickTwoPaths } from './dropPaths';
import { SessionTreePanel } from './SessionTreePanel';
import { S } from '../../strings';

/**
 * 主页视图（FR-HOME-001，BC 风格）：
 * - 左侧会话树面板（保存的会话/最近会话/新建分支，FR-HOME-002，F9 可切换显隐）；
 * - 中央六类会话类型大按钮区（未实现项置灰 + 阶段 tooltip）；
 * - 最近打开列表（MRU 路径，点击即开）；路径输入 + 开始比较；
 * - 拖放：拖入两个文件夹自动填左右路径；拖到"文件夹比较"大按钮直接开比较。
 */
export function HomeView() {
  const openFolderTab = useShell((st) => st.openFolderTab);
  const openFolderSyncTab = useShell((st) => st.openFolderSyncTab);
  const openPictureTab = useShell((st) => st.openPictureTab);
  const openFolderMergeTab = useShell((st) => st.openFolderMergeTab);
  const openTextTab = useShell((st) => st.openTextTab);
  const setNotice = useShell((st) => st.setNotice);
  const [showPanel, setShowPanel] = useState(() => sessionService.getUiState().showSessionPanel !== false);
  const [mruPaths, setMruPaths] = useState(() => sessionService.getMruPaths('folder-compare'));
  const [left, setLeft] = useState('');
  const [right, setRight] = useState('');
  const [dragOver, setDragOver] = useState(false);

  useEffect(() => {
    const refresh = () => {
      setMruPaths(sessionService.getMruPaths('folder-compare'));
      setShowPanel(sessionService.getUiState().showSessionPanel !== false);
    };
    return sessionService.subscribe(refresh);
  }, []);

  const fillAndMaybeStart = (l: string, r: string, start: boolean) => {
    setLeft(l);
    setRight(r);
    if (start && l && r) {
      openFolderTab(l, r);
      setLeft('');
      setRight('');
    }
  };

  const start = () => {
    if (!left.trim() || !right.trim()) {
      setNotice(S.home.pathRequired);
      return;
    }
    openFolderTab(left.trim(), right.trim());
    setLeft('');
    setRight('');
  };

  const startKind = (kind: SessionKind) => {
    const meta = KIND_META[kind];
    if (!meta.implemented) {
      setNotice(`${meta.title}视图属 ${meta.phase} 阶段交付`);
      return;
    }
    if (kind === 'folder-compare') {
      if (left.trim() && right.trim()) start();
      else setNotice(S.home.pathRequired);
      return;
    }
    if (kind === 'text-compare') {
      if (left.trim() && right.trim()) void openTextTab(left.trim(), right.trim());
      else setNotice(S.home.textPathRequired);
      return;
    }
    // P8：文件夹同步复用左右两路径；文件夹合并三路以左路径兼任基准
    if (kind === 'picture-compare') {
      if (left.trim() && right.trim()) openPictureTab(left.trim(), right.trim());
      else setNotice(S.home.pathRequired);
      return;
    }
    if (kind === 'folder-sync') {
      if (left.trim() && right.trim()) openFolderSyncTab(left.trim(), right.trim());
      else setNotice(S.home.pathRequired);
      return;
    }
    if (kind === 'folder-merge') {
      if (left.trim() && right.trim()) openFolderMergeTab(left.trim(), left.trim(), right.trim());
      else setNotice(S.home.pathRequired);
      return;
    }
  };

  const onDrop = async (e: React.DragEvent, kind?: SessionKind) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
    const paths = await extractPathsFromDataTransfer(e.dataTransfer);
    const pair = pickTwoPaths(paths);
    if (pair.left) setLeft(pair.left);
    if (pair.right) setRight(pair.right);
    if (pair.warning) setNotice(pair.warning);
    if (pair.left && pair.right) {
      if (kind === 'text-compare') void openTextTab(pair.left, pair.right);
      else if (kind === 'picture-compare') openPictureTab(pair.left, pair.right);
      else if (kind === 'folder-sync') openFolderSyncTab(pair.left, pair.right);
      else if (kind === 'folder-merge') openFolderMergeTab(pair.left, pair.left, pair.right);
      else openFolderTab(pair.left, pair.right);
    }
  };

  return (
    <div
      className={`aic-home aic-home-v2${dragOver ? ' drag-over' : ''}`}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => void onDrop(e)}
    >
      {showPanel && <SessionTreePanel />}

      <div className="aic-home-main">
        <h1>{S.home.title}</h1>
        <div className="sub">{S.home.subtitleV2}</div>

        <div className="aic-type-grid">
          {viewRegistry
            .listByOrder()
            .filter((p): p is ViewPlugin & { id: SessionKind } => p.id !== 'home')
            .map((p) => (
              <button
                key={p.id}
                className={`aic-type-btn${p.implemented ? '' : ' disabled'}`}
                disabled={!p.implemented}
                title={p.implemented ? `${p.title}（可拖入两个路径直接开始）` : (p.phaseNote ?? '')}
                onClick={() => startKind(p.id)}
                onDrop={(e) => void onDrop(e, p.id)}
                onDragOver={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                }}
              >
                <span className="icon">{p.icon}</span>
                <span className="label">{p.title}</span>
              </button>
            ))}
        </div>

        <div className="aic-home-row">
          <label style={{ width: 84, alignSelf: 'center' }}>{S.home.leftPath}</label>
          <input
            value={left}
            placeholder={S.home.leftPlaceholder}
            onChange={(e) => setLeft(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && start()}
          />
        </div>
        <div className="aic-home-row">
          <label style={{ width: 84, alignSelf: 'center' }}>{S.home.rightPath}</label>
          <input
            value={right}
            placeholder={S.home.rightPlaceholder}
            onChange={(e) => setRight(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && start()}
          />
        </div>
        <div className="aic-home-actions">
          <button
            title={S.home.sample}
            onClick={() => {
              setLeft(S.home.sampleLeft);
              setRight(S.home.sampleRight);
            }}
          >
            {S.home.sample}
          </button>
          <button className="primary" onClick={start}>
            {S.home.start}
          </button>
        </div>

        <div className="aic-recent">
          <div className="aic-tree-head">{S.home.recentTitle}</div>
          {mruPaths.left.length === 0 && mruPaths.right.length === 0 && (
            <div className="aic-tree-empty">{S.home.recentEmpty}</div>
          )}
          {mruPaths.left.map((l, i) => {
            const r = mruPaths.right[i];
            return (
              <button
                key={l}
                className="aic-recent-row"
                title={`${l} ⇄ ${r ?? ''}`}
                onClick={() => fillAndMaybeStart(l, r ?? '', !!r)}
              >
                📁 <span className="pair">{l} ⇄ {r ?? '—'}</span>
              </button>
            );
          })}
        </div>
      </div>

      {dragOver && (
        <div className="aic-drop-overlay">{S.home.dropHint}</div>
      )}
    </div>
  );
}
