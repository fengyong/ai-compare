import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { DEFAULT_DIR_CRITERIA, type DirRow } from '../../protocol/types';
import type { Tab } from '../../shell/store';
import { useShell } from '../../shell/store';
import { openFolder, trackSession } from '../../ipc/engineProcess';
import { formatSize, formatTime } from '../../lib/env';
import { S } from '../../strings';
import { ensureDirCacheAttached } from './dirRowCache';
import { DirStatsBar } from './DirStatsBar';
import { statusDotClass, statusVisual, toneClass } from './statusDisplay';

/**
 * 文件夹比较视图（docs/03 §3.4.2 folder-compare 的 P0 子集 / U4）：
 * - 虚拟滚动表（TanStack Virtual）：10 万行目标级窗口渲染（PB-04）
 * - 数据面：dir/open（客户端分配 sessionId）→ 引擎按批推 dir/delta 全量行
 *   （ADR-003/ADR-004），缓存按行 id 幂等合并（dirRowCache.ts）
 * - 状态着色对齐 docs/01 附录 C.1；双击文件行打开文本子会话（FR-DIR-070）
 * - 选择/文件操作/列配置/树展开属 P6（引擎 dir/expand / dir/rows P1 开放）
 */
export function FolderCompareView({ tab }: { tab: Tab }) {
  const folder = tab.folder!;
  const openTextChild = useShell((st) => st.openTextChild);
  const setNotice = useShell((st) => st.setNotice);

  // sessionId 由客户端分配（生成协议 DirOpenParams），标签创建时即确定；
  // delta 前送在 dir/open 之前挂好，无应答/首批通知的竞态
  const cache = ensureDirCacheAttached(folder.sessionId!);

  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);

  const version = useSyncExternalStore(cache.subscribe, cache.getSnapshot);

  // ---- 会话建立（ensureOpened 幂等：StrictMode 双调用安全） ----
  useEffect(() => {
    let disposed = false;
    void cache
      .ensureOpened(() =>
        openFolder({
          sessionId: cache.sessionId,
          left: folder.left,
          right: folder.right,
          depth: 1,
          criteria: DEFAULT_DIR_CRITERIA,
          // P2 新参数保持 P0 行为（同步扫描、无过滤器、树形对齐）
          lazyScan: null,
          flat: null,
          nameFilter: null,
          otherFilter: null,
          displayFilter: null,
          manualAlign: null,
        })
      )
      .then(() => {
        if (!disposed) {
          trackSession(cache.sessionId, 'dir', {
            left: folder.left,
            right: folder.right,
            depth: 1,
            criteria: DEFAULT_DIR_CRITERIA,
            lazyScan: null,
            flat: null,
            nameFilter: null,
            otherFilter: null,
            displayFilter: null,
            manualAlign: null,
          });
          setReady(true);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
  }, [cache, folder.left, folder.right]);

  const snapshot = cache.getStats();

  // ---- 虚拟滚动 ----
  const scrollRef = useRef<HTMLDivElement>(null);
  const total = ready ? snapshot.total : 0;
  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 28,
    overscan: 12,
  });
  const items = virtualizer.getVirtualItems();

  useLayoutEffect(() => {
    virtualizer.measure();
  }, [virtualizer, total]);

  const openRow = useMemo(
    () => (index: number) => {
      const row = cache.getRow(index);
      if (!row) return;
      if (row.kind !== 'file') {
        setNotice(S.folder.dirNotSupported);
        return;
      }
      if (row.status === 'leftOnly' || row.status === 'rightOnly') {
        setNotice(S.folder.orphanHint);
        return;
      }
      void openTextChild(tab.id, row);
    },
    [cache, openTextChild, tab.id, setNotice]
  );

  if (error) {
    return (
      <div className="aic-folder">
        <div className="aic-error-box">
          <b>文件夹比较打开失败</b>
          <pre>{error}</pre>
          <button onClick={() => setError(null)}>关闭</button>
        </div>
      </div>
    );
  }

  return (
    <div className="aic-folder">
      <div className="aic-folder-toolbar">
        <span className="aic-folder-paths" title={`${folder.left} ⇄ ${folder.right}`}>
          {folder.left} ⇄ {folder.right}
        </span>
        {ready ? (
          <DirStatsBar snapshot={snapshot} />
        ) : (
          <span style={{ color: 'var(--aic-text-dim)' }}>{S.engine.connecting}</span>
        )}
      </div>

      <div className="aic-dir-header">
        <span>{S.folder.nameCol}</span>
        <span className="r">{S.folder.leftTag} · {S.folder.sizeCol}</span>
        <span className="r">{S.folder.mtimeCol}</span>
        <span className="c">{S.folder.contentCol}</span>
        <span className="r">{S.folder.rightTag} · {S.folder.sizeCol}</span>
        <span className="r">{S.folder.mtimeCol}</span>
      </div>

      <div className="aic-dir-scroll" ref={scrollRef} data-version={version}>
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative', minWidth: 720 }}>
          {items.map((item) => {
            const row = cache.getRow(item.index);
            const selected = selectedIndex === item.index;
            return (
              <div
                key={row?.id ?? item.key}
                className={`aic-dir-row${selected ? ' selected' : ''}`}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  height: item.size,
                  transform: `translateY(${item.start}px)`,
                }}
                onClick={() => setSelectedIndex(item.index)}
                onDoubleClick={() => openRow(item.index)}
              >
                {row ? (
                  <DirRowCells row={row} />
                ) : (
                  <>
                    <span className="aic-dir-cell placeholder">…</span>
                    <span className="aic-dir-cell placeholder r">…</span>
                    <span className="aic-dir-cell placeholder r">…</span>
                    <span className="aic-dir-cell placeholder c">…</span>
                    <span className="aic-dir-cell placeholder r">…</span>
                    <span className="aic-dir-cell placeholder r">…</span>
                  </>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** 行缩进深度：行 id 为 POSIX 风格相对路径（生成协议 DirRow.id） */
function rowDepth(row: DirRow): number {
  let depth = 0;
  for (const ch of row.id) if (ch === '/') depth++;
  return depth;
}

function DirRowCells({ row }: { row: DirRow }) {
  const vis = statusVisual(row.status);
  const isDir = row.kind === 'dir';
  return (
    <>
      <span
        className={`aic-dir-cell name ${toneClass(vis.tone)}`}
        title={`${row.id} · ${vis.label}`}
        style={{ paddingLeft: 12 + rowDepth(row) * 18 }}
      >
        <i className={`aic-dot ${statusDotClass[vis.tone]}`} />
        <span className="fname">
          {isDir ? '▸ ' : ''}
          {row.name}
        </span>
      </span>
      <span className="aic-dir-cell mono r">{formatSize(row.left?.size)}</span>
      <span className="aic-dir-cell mono r">{formatTime(row.left?.mtimeMs)}</span>
      <span className={`aic-dir-cell c aic-center-icon ${vis.centerClass}`} title={vis.label}>
        {vis.centerGlyph}
      </span>
      <span className="aic-dir-cell mono r">{formatSize(row.right?.size)}</span>
      <span className="aic-dir-cell mono r">{formatTime(row.right?.mtimeMs)}</span>
    </>
  );
}
