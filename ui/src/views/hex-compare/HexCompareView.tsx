import { useCallback, useEffect, useRef, useState } from 'react';
import { getEngineCapabilities, hexBytes, openHex } from '../../ipc/engineProcess';
import { S } from '../../strings';
import type { Tab } from '../../shell/store';

/**
 * 十六进制比较视图（P14 消费 hex/open|bytes，FR-HEX）。
 *
 * BC 语义：左右双栏 16 字节/行（地址 + hex 对 + ASCII 侧栏）、
 * 逐字节状态着色（=同/x 变/+插/-删）、滚动分页（4096 字节/页）、统计栏、
 * 偏移跳转输入。
 */

interface HexLine {
  addr: number;
  hex: string;
  ascii: string;
  status: string;
}

interface Summary {
  sameBytes: number;
  changedBytes: number;
  insertedBytes: number;
  deletedBytes: number;
}

/** 单页字节预算（引擎单页上限 4096B = 256 行） */
const PAGE_BYTES = 4096;
/** 每行 16 字节 */
const ROW_BYTES = 16;

/** 逐字节状态字符 -> 着色类（= 同/x 变/+ 插/- 删；纯函数可测） */
export function statusCharCls(c: string): string {
  switch (c) {
    case 'x':
      return 'hx-changed';
    case '+':
      return 'hx-inserted';
    case '-':
      return 'hx-deleted';
    default:
      return 'hx-same';
  }
}

/** 状态字符统计（测试锚点） */
export function countStatus(status: string): { same: number; changed: number; inserted: number; deleted: number } {
  let same = 0;
  let changed = 0;
  let inserted = 0;
  let deleted = 0;
  for (const c of status) {
    if (c === 'x') changed++;
    else if (c === '+') inserted++;
    else if (c === '-') deleted++;
    else same++;
  }
  return { same, changed, inserted, deleted };
}

/** 渲染一行 hex 单元格数组（hex 对 + 每字节着色 span 数据；纯函数可测） */
export function hexRowCells(line: HexLine): Array<{ text: string; cls: string }> {
  const texts = line.hex.split(' ');
  return texts.map((text, i) => ({
    text,
    cls: statusCharCls(line.status[i] ?? '='),
  }));
}

function HexPane({
  side,
  sessionId,
  total,
  jumpTo,
}: {
  side: 'left' | 'right';
  sessionId: string;
  total: number;
  jumpTo: number | null;
}) {
  const [lines, setLines] = useState<HexLine[]>([]);
  const [loading, setLoading] = useState(false);
  const [done, setDone] = useState(false);
  const hostRef = useRef<HTMLDivElement | null>(null);
  const offsetRef = useRef(0);
  const jumpRequested = useRef<number | null>(null);
  // 分页在途标记放 ref：loading 是 state，进 useCallback 依赖会重建闭包，
  // 而滚动回调持有的旧闭包读到恒为 false 的 loading → 并发同 offset 拉页 → 重复行 + 偏移漂移
  const inFlightRef = useRef(false);

  const loadNext = useCallback(async () => {
    if (inFlightRef.current || done) return;
    inFlightRef.current = true;
    setLoading(true);
    try {
      const r = await hexBytes({ sessionId, side, offset: offsetRef.current, length: PAGE_BYTES });
      setLines((prev) => [...prev, ...r.lines]);
      offsetRef.current += r.lines.length * ROW_BYTES;
      if (r.done || r.lines.length === 0) setDone(true);
      // 跳转请求：滚到目标行
      if (jumpRequested.current !== null) {
        const idx = Math.floor(jumpRequested.current / ROW_BYTES);
        const el = hostRef.current?.querySelector(`[data-row="${idx}"]`);
        el?.scrollIntoView({ block: 'start' });
        jumpRequested.current = null;
      }
    } finally {
      inFlightRef.current = false;
      setLoading(false);
    }
  }, [sessionId, side, done]);

  useEffect(() => {
    void loadNext();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, side]);

  // 偏移跳转：重置分页从对齐行首重拉
  useEffect(() => {
    if (jumpTo === null) return;
    const aligned = Math.floor(jumpTo / ROW_BYTES) * ROW_BYTES;
    offsetRef.current = aligned;
    setLines([]);
    setDone(false);
    void hexBytes({ sessionId, side, offset: aligned, length: PAGE_BYTES }).then((r) => {
      setLines(r.lines);
      offsetRef.current = aligned + r.lines.length * ROW_BYTES;
      if (r.done) setDone(true);
      const idx = Math.floor((jumpTo - aligned) / ROW_BYTES);
      requestAnimationFrame(() => {
        hostRef.current
          ?.querySelector(`[data-row="${Math.floor(jumpTo / ROW_BYTES) - idx + Math.floor(jumpTo / ROW_BYTES)}"]`)
          ?.scrollIntoView({ block: 'start' });
      });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jumpTo]);

  const onScroll = useCallback(() => {
    const host = hostRef.current;
    if (!host) return;
    if (host.scrollTop + host.clientHeight >= host.scrollHeight - 60) void loadNext();
  }, [loadNext]);

  return (
    <div className="aic-hex-pane" ref={hostRef} onScroll={onScroll} data-side={side}>
      <table className="aic-hex-inner">
        <tbody>
          {lines.map((line, i) => {
            const cells = hexRowCells(line);
            return (
              <tr key={line.addr} data-row={i}>
                <td className="hx-addr">{line.addr.toString(16).padStart(8, '0')}</td>
                {cells.map((c, k) => (
                  <td key={k} className={`hx-byte ${c.cls}`}>{c.text}</td>
                ))}
                <td className="hx-ascii">{line.ascii}</td>
              </tr>
            );
          })}
          {!done && loading && (
            <tr>
              <td colSpan={18} className="hx-loading">{S.hex.loading}</td>
            </tr>
          )}
        </tbody>
      </table>
      <span className="aic-hex-total" style={{ display: 'none' }}>{total}</span>
    </div>
  );
}

export function HexCompareView({ tab }: { tab: Tab & { kind: 'hex'; hex: { left: string; right: string; sessionId: string } } }) {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [sizes, setSizes] = useState<{ leftSize: number; rightSize: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [jumpTo, setJumpTo] = useState<number | null>(null);
  const [jumpInput, setJumpInput] = useState('');

  useEffect(() => {
    let disposed = false;
    openHex({ sessionId: tab.hex.sessionId, left: tab.hex.left, right: tab.hex.right, options: null })
      .then((r) => {
        if (!disposed) {
          setSummary(r.summary);
          setSizes({ leftSize: r.leftSize, rightSize: r.rightSize });
        }
      })
      .catch((err) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.hex.sessionId]);

  const doJump = () => {
    const n = Number.parseInt(jumpInput.replace(/^0x/i, ''), 16);
    if (!Number.isNaN(n)) setJumpTo(n);
  };

  if (error) {
    return <div className="aic-hex-error">{S.hex.loadFail}: {error}</div>;
  }

  return (
    <div className="aic-hex">
      <div className="aic-hex-toolbar">
        <span className="aic-hex-title">{S.hex.title}: {tab.title.replace('十六进制比较：', '')}</span>
        {!getEngineCapabilities()?.hexCompare && <span className="aic-hex-cap">（{S.hex.capNote}）</span>}
        <label>
          {S.hex.jump}
          <input
            className="aic-hex-jump"
            value={jumpInput}
            placeholder="0x0000"
            onChange={(e) => setJumpInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') doJump();
            }}
          />
          <button onClick={doJump}>{S.hex.jumpGo}</button>
        </label>
      </div>
      {summary && (
        <div className="aic-hex-stats">
          <span>{S.hex.same}: {summary.sameBytes}</span>
          <span>{S.hex.changed}: {summary.changedBytes}</span>
          <span>{S.hex.inserted}: {summary.insertedBytes}</span>
          <span>{S.hex.deleted}: {summary.deletedBytes}</span>
          {sizes && <span>{S.hex.sizes}: {sizes.leftSize} ⇄ {sizes.rightSize} B</span>}
        </div>
      )}
      <div className="aic-hex-host">
        <div className="aic-hex-half">
          <div className="aic-hex-pane-title">{S.hex.leftPane}</div>
          <HexPane side="left" sessionId={tab.hex.sessionId} total={sizes?.leftSize ?? 0} jumpTo={jumpTo} />
        </div>
        <div className="aic-hex-half">
          <div className="aic-hex-pane-title">{S.hex.rightPane}</div>
          <HexPane side="right" sessionId={tab.hex.sessionId} total={sizes?.rightSize ?? 0} jumpTo={jumpTo} />
        </div>
      </div>
    </div>
  );
}
