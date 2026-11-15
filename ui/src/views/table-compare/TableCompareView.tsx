import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  getEngineCapabilities,
  openTableIdempotent,
  tableSections,
  trackSession,
} from '../../ipc/engineProcess';
import { S } from '../../strings';
import type { Tab } from '../../shell/store';

/**
 * 表格比较视图（P14 消费 table/open|sections，FR-TBL-001~004）。
 *
 * BC 语义：比较列网格（列对齐后的逻辑列）、沟槽行类别点
 * （红点=important 差异、蓝点=不重要差异）、单元格状态着色
 * （重要差异红底/不重要蓝底/单侧缺失灰）、统计栏。
 * 行分页取数（table/sections），表头固定。
 */

interface TableRow {
  kind: 'same' | 'changed' | 'added' | 'removed';
  left: number | null;
  right: number | null;
  important: boolean;
  cells: Array<{ left: string | null; right: string | null; status: string }>;
}

/** 行类别 -> 沟槽点样式（BC 语义：红点/蓝点/绿加/灰删；纯函数可测） */
export function rowDot(kind: string, important: boolean): { cls: string; title: string } {
  switch (kind) {
    case 'changed':
      return important
        ? { cls: 'dot-red', title: S.table.dotChanged }
        : { cls: 'dot-blue', title: S.table.dotUnimportant };
    case 'added':
      return { cls: 'dot-green', title: S.table.dotAdded };
    case 'removed':
      return { cls: 'dot-gray', title: S.table.dotRemoved };
    default:
      return { cls: 'dot-none', title: '' };
  }
}

/** 单元格状态 -> CSS 类（红底=重要差异、蓝底=不重要、灰=单侧缺失） */
export function cellCls(status: string): string {
  switch (status) {
    case 'importantDiff':
      return 'cell-important';
    case 'unimportantDiff':
      return 'cell-unimportant';
    case 'leftOnly':
    case 'rightOnly':
      return 'cell-oneside';
    default:
      return '';
  }
}

/** 统计栏格式化（测试锚点） */
export function formatSummary(s: {
  sameRows: number;
  changedRows: number;
  addedRows: number;
  removedRows: number;
  unimportantRows: number;
}): string {
  return `${s.sameRows} 同 / ${s.changedRows} 异 / ${s.addedRows} 加 / ${s.removedRows} 删 / ${s.unimportantRows} 蓝点`;
}

const PAGE = 200;

export function TableCompareView({
  tab,
}: {
  tab: Tab & { kind: 'table'; table: { left: string; right: string; sessionId: string } };
}) {
  const [columns, setColumns] = useState<
    Array<{ name: string; ty: string; left: number | null; right: number | null }>
  >([]);
  const [rows, setRows] = useState<TableRow[]>([]);
  const [summary, setSummary] = useState<{
    sameRows: number;
    changedRows: number;
    addedRows: number;
    removedRows: number;
    unimportantRows: number;
  } | null>(null);
  const [rowCount, setRowCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const loadingRef = useRef(false);
  const hostRef = useRef<HTMLDivElement | null>(null);

  const loadPage = useCallback(
    async (offset: number): Promise<TableRow[]> => {
      const r = await tableSections({ sessionId: tab.table.sessionId, offset, count: PAGE });
      return r.rows;
    },
    [tab.table.sessionId],
  );

  useEffect(() => {
    let disposed = false;
    openTableIdempotent({
      sessionId: tab.table.sessionId,
      left: tab.table.left,
      right: tab.table.right,
      options: null,
    })
      .then((opened) => {
        if (disposed) return;
        trackSession(tab.table.sessionId, 'table', {
          left: tab.table.left,
          right: tab.table.right,
          options: null,
        });
        setColumns(opened.columns);
        setRowCount(opened.rowCount);
        setSummary(opened.summary);
        setReady(true);
      })
      .catch((err) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.table.sessionId]);

  // 首页 + 滚动分页（windowed 增量追加）
  useEffect(() => {
    if (!ready) return;
    let disposed = false;
    loadPage(0)
      .then((first) => {
        if (!disposed) setRows(first);
      })
      .catch((err) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
  }, [ready, loadPage]);

  const onScroll = useCallback(() => {
    const host = hostRef.current;
    if (!host || loadingRef.current) return;
    if (rows.length >= rowCount) return;
    if (host.scrollTop + host.clientHeight < host.scrollHeight - 200) return;
    loadingRef.current = true;
    loadPage(rows.length)
      .then((next) => {
        setRows((prev) => [...prev, ...next]);
        loadingRef.current = false;
      })
      .catch(() => {
        loadingRef.current = false;
      });
  }, [rows.length, rowCount, loadPage]);

  const statsText = useMemo(() => (summary ? formatSummary(summary) : ''), [summary]);

  if (error) {
    return (
      <div className="aic-table-error">
        {S.table.loadFail}: {error}
      </div>
    );
  }

  return (
    <div className="aic-table">
      <div className="aic-table-toolbar">
        <span className="aic-table-title">
          {S.table.title}: {tab.title.replace('表格比较：', '')}
        </span>
        {!getEngineCapabilities()?.tableCompare && (
          <span className="aic-table-cap">（mock 演示数据）</span>
        )}
      </div>
      {summary && <div className="aic-table-stats">{statsText}</div>}
      <div className="aic-table-grid" ref={hostRef} onScroll={onScroll}>
        <table className="aic-table-inner">
          <thead>
            <tr>
              <th className="aic-table-gutter" />
              {columns.map((c, i) => (
                <th key={i} title={`${c.ty} 列`}>
                  {c.name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => {
              const dot = rowDot(row.kind, row.important);
              return (
                <tr key={i} className={`aic-table-row kind-${row.kind}`}>
                  <td className={`aic-table-gutter ${dot.cls}`} title={dot.title} />
                  {row.cells.map((cell, ci) => (
                    <td
                      key={ci}
                      className={cellCls(cell.status)}
                      title={`${cell.left ?? '—'} ⇄ ${cell.right ?? '—'}`}
                    >
                      {cell.right ?? cell.left ?? ''}
                    </td>
                  ))}
                </tr>
              );
            })}
            {rows.length < rowCount && (
              <tr>
                <td className="aic-table-loading" colSpan={columns.length + 1}>
                  {S.table.loading}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

void S;
