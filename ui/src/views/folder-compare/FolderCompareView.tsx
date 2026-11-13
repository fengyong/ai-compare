import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { DirRow } from '../../protocol/types';
import type { Tab } from '../../shell/store';
import { useShell } from '../../shell/store';
import { getDirRows, openFolder } from '../../ipc/engineProcess';
import { formatSize, formatTime } from '../../lib/env';
import { S } from '../../strings';
import { ensureDirCacheAttached, type DirRowCache } from './dirRowCache';
import { DirStatsBar } from './DirStatsBar';
import { effectiveStatus, statusDotClass, statusVisual, toneClass } from './statusDisplay';

/**
 * 文件夹比较视图（docs/03 §3.4.2 folder-compare 的 P0 子集 / U4）：
 * - 虚拟滚动表（TanStack Virtual）：10 万行目标级窗口渲染（PB-04）
 * - 窗口化取数：滚动到哪取哪（dir/rows）+ dir/delta 增量合并（ADR-004/ADR-003）
 * - 状态着色对齐 docs/01 附录 C.1；双击文件行打开文本子会话（FR-DIR-070）
 * - 选择/文件操作/列配置/树展开属 P6
 */
export function FolderCompareView({ tab }: { tab: Tab }) {
  const folder = tab.folder!;
  const openTextChild = useShell((st) => st.openTextChild);
  const setNotice = useShell((st) => st.setNotice);

  const cacheRef = useRef<DirRowCache | null>(null);
  const [, setReady] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);

  const version = useSyncExternalStore(
    cacheRef.current?.subscribe ?? ((fn) => {
      void fn;
      return () => undefined;
    }),
    cacheRef.current?.getSnapshot ?? (() => 0)
  );

  // ---- 会话建立（delta 前送挂在会话缓存上，见 dirRowCache.ensureDirCacheAttached） ----
  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        let sessionId = tab.folder?.sessionId;
        if (!sessionId) {
          const opened = await openFolder({ left: folder.left, right: folder.right });
          if (disposed) return;
          sessionId = opened.sessionId;
          useShell.getState().setFolderSession(tab.id, sessionId);
        }
        const cache = ensureDirCacheAttached(sessionId);
        cacheRef.current = cache;
        setReady((x) => x + 1);
      } catch (err) {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      disposed = true;
    };
  }, [tab.id, folder.left, folder.right, tab.folder?.sessionId]);

  const cache = cacheRef.current;
  const snapshot = cache?.getStats();

  // ---- 虚拟滚动 ----
  const scrollRef = useRef<HTMLDivElement>(null);
  const total = snapshot?.total ?? 0;
  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 28,
    overscan: 12,
  });
  const items = virtualizer.getVirtualItems();

  // 可视范围变化 → 窗口取数（预取上下各 1 屏）
  const rangeKey = items.length ? `${items[0].index}:${items[items.length - 1].index}:${version}` : '';
  const sessionId = cache?.sessionId;
  useEffect(() => {
    if (!cache || !items.length || !sessionId) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    const span = last - first + 1;
    void cache.ensureWindow(Math.max(0, first - span), span * 3, (o, c) =>
      getDirRows({ sessionId, offset: o, count: c })
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- items 数组引用每次渲染都会变，用 rangeKey 代替
  }, [rangeKey, cache, sessionId]);

  useLayoutEffect(() => {
    virtualizer.measure();
  }, [virtualizer, total]);

  const openRow = useMemo(
    () => (index: number) => {
      const row = cache?.getRow(index);
      if (!row || !sessionId) return;
      if (row.kind !== 'file') {
        setNotice(S.folder.dirNotSupported);
        return;
      }
      if (effectiveStatus(row) === 'orphanLeft' || effectiveStatus(row) === 'orphanRight') {
        setNotice(S.folder.orphanHint);
        return;
      }
      void openTextChild(tab.id, row);
    },
    [cache, sessionId, openTextChild, tab.id, setNotice]
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
        {snapshot ? (
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

      <div className="aic-dir-scroll" ref={scrollRef}>
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative', minWidth: 720 }}>
          {items.map((item) => {
            const row = cache?.getRow(item.index);
            const selected = selectedIndex === item.index;
            return (
              <div
                key={item.key}
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

function DirRowCells({ row }: { row: DirRow }) {
  const status = effectiveStatus(row);
  const vis = statusVisual(status);
  const isDir = row.kind === 'dir' || row.kind === 'archive';
  return (
    <>
      <span
        className={`aic-dir-cell name ${toneClass(vis.tone)}`}
        title={`${row.relPath} · ${vis.label}`}
        style={{ paddingLeft: 12 + row.depth * 18 }}
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
