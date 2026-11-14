import { useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent, WheelEvent as ReactWheelEvent } from 'react';
import { openPicture } from '../../ipc/engineProcess';
import { S } from '../../strings';
import type { Tab } from '../../shell/store';
import {
  ZOOM_STEP,
  actualZoom,
  autoScaleGeometry,
  blendPercent,
  blinkSide,
  fitScale,
  offsetStep,
  panTransform,
  zoomBy,
  zoomIn,
  zoomOut,
  type BlinkMode,
  type ZoomState,
} from './pictureViewModel';

/**
 * 图片比较视图（P14 基线 + 队列 7；docs/01 §3.8 FR-PIX-001~004）。
 *
 * 数据面：picture/open（引擎像素容差 diff → 网格块聚合；PictureDiffResult）。
 * 渲染：左右双窗格（演示模式为程序化 canvas 图案，真实模式为 file:// 图片）
 * + 差异块叠加 canvas；两窗格共用同一 ZoomState 实现缩放/平移同步。
 *
 * 队列 7 交互：
 * - FR-PIX-002 混合/闪烁：blink 定时切换左右，blend 三角波混合百分比；
 * - FR-PIX-003 自动缩放（较小图匹配较大图比例）+ 差异偏移（箭头键/Ctrl 大步）；
 * - FR-PIX-004 放大/缩小/缩放到适合/实际像素 + Ctrl+滚轮缩放 + 拖动平移同步。
 */

interface PictureOpenData {
  sessionId: string;
  width: number;
  height: number;
  leftWidth: number;
  leftHeight: number;
  rightWidth: number;
  rightHeight: number;
  diffPixels: number;
  totalPixels: number;
  similarityPct: number;
  blocks: Array<{ x: number; y: number; w: number; h: number; diffRatio: number }>;
  truncated: boolean;
}

/** 统计值格式化（测试锚点：相似度保留两位、像素千分位） */
export function formatStats(d: Pick<PictureOpenData, 'similarityPct' | 'diffPixels' | 'totalPixels'>): string {
  const sim = d.similarityPct.toFixed(2);
  const diff = d.diffPixels.toLocaleString('en-US');
  const total = d.totalPixels.toLocaleString('en-US');
  return `${sim}% (${diff} / ${total} px)`;
}

/** 差异块 -> canvas 叠加矩形（含占比透明度；纯函数可测） */
export function blockRects(
  blocks: Array<{ x: number; y: number; w: number; h: number; diffRatio: number }>,
  scale: number
): Array<{ x: number; y: number; w: number; h: number; alpha: number }> {
  return blocks.map((b) => ({
    x: b.x * scale,
    y: b.y * scale,
    w: b.w * scale,
    h: b.h * scale,
    alpha: 0.05 + Math.min(0.55, b.diffRatio * 0.5),
  }));
}

/** 演示模式合成图案（确定性种子；mock 引擎响应同种子同块布局） */
function drawDemoPattern(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  seed: number,
  side: 'left' | 'right'
): void {
  let rnd = ((seed * 2654435761) % 4294967296) || 1;
  ctx.fillStyle = '#141414';
  ctx.fillRect(0, 0, w, h);
  for (let i = 0; i < 24; i++) {
    rnd = (rnd * 1103515245 + 12345) % 2147483648;
    const x = rnd % w;
    rnd = (rnd * 1103515245 + 12345) % 2147483648;
    const y = rnd % h;
    rnd = (rnd * 1103515245 + 12345) % 2147483648;
    const size = 24 + (rnd % 96);
    rnd = (rnd * 1103515245 + 12345) % 2147483648;
    const hue = rnd % 360;
    ctx.fillStyle = side === 'left' ? `hsl(${hue} 60% 45%)` : `hsl(${(hue + 40) % 360} 60% 45%)`;
    ctx.fillRect(x, y, size, size);
  }
}

type PictureTab = Tab & { kind: 'picture'; picture: { left: string; right: string; sessionId: string } };

export function PictureCompareView({ tab }: { tab: PictureTab }) {
  const [data, setData] = useState<PictureOpenData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tolerance, setTolerance] = useState(8);
  const [zoom, setZoom] = useState<ZoomState>(actualZoom());
  const [autoScale, setAutoScale] = useState(false);
  const [blinkMode, setBlinkMode] = useState<BlinkMode>('off');
  const [blinkMs, setBlinkMs] = useState(400);
  const [tick, setTick] = useState(0);
  const [offset, setOffset] = useState({ dx: 0, dy: 0 });
  const [panStart, setPanStart] = useState<{ x: number; y: number; panX: number; panY: number } | null>(null);
  const hostRef = useRef<HTMLDivElement | null>(null);
  const leftCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const rightCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const diffCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const demoSeed = tab.picture.left.length + tab.picture.right.length;
  const isDemo = tab.picture.left.startsWith('/demo/') || tab.picture.right.startsWith('/demo/');

  useEffect(() => {
    let disposed = false;
    openPicture({
      sessionId: tab.picture.sessionId,
      left: tab.picture.left,
      right: tab.picture.right,
      options: { tolerance, block: 32 },
    })
      .then((r) => {
        if (!disposed) setData(r);
      })
      .catch((err) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
    // 容差变化触发重比较（sessionId 复用会被 411 拒——演示模式忽略）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.picture.sessionId, tolerance]);

  // 闪烁/混合节拍（off 时停表）
  useEffect(() => {
    if (blinkMode === 'off') return;
    const id = window.setInterval(() => setTick((t) => t + 1), Math.max(60, blinkMs));
    return () => window.clearInterval(id);
  }, [blinkMode, blinkMs]);

  // 演示图案：左右两侧各一张确定性图案
  useEffect(() => {
    if (!isDemo || !data) return;
    for (const [ref, side] of [
      [leftCanvasRef, 'left'],
      [rightCanvasRef, 'right'],
    ] as const) {
      const canvas = ref.current;
      if (!canvas) continue;
      canvas.width = data.width;
      canvas.height = data.height;
      const ctx = canvas.getContext('2d');
      if (ctx) drawDemoPattern(ctx, data.width, data.height, demoSeed, side);
    }
  }, [isDemo, data, demoSeed]);

  // 差异块叠加（按原始画布坐标绘制；缩放由 CSS transform 承担）
  useEffect(() => {
    const canvas = diffCanvasRef.current;
    if (!canvas || !data) return;
    canvas.width = data.width;
    canvas.height = data.height;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    for (const r of blockRects(data.blocks, 1)) {
      ctx.fillStyle = `rgba(220, 40, 40, ${r.alpha})`;
      ctx.fillRect(r.x, r.y, r.w, r.h);
      ctx.strokeStyle = 'rgba(220, 40, 40, 0.9)';
      ctx.lineWidth = 1;
      ctx.strokeRect(r.x + 0.5, r.y + 0.5, r.w - 1, r.h - 1);
    }
  }, [data]);

  const stats = useMemo(() => (data ? formatStats(data) : S.picture.loading), [data]);

  const geometry = useMemo(
    () => data
      ? autoScaleGeometry(
          { w: data.leftWidth, h: data.leftHeight },
          { w: data.rightWidth, h: data.rightHeight }
        )
      : { left: 1, right: 1 },
    [data]
  );

  const transformFor = (side: 'left' | 'right'): string => {
    const z = autoScale ? zoomBy(zoom, geometry[side]) : zoom;
    return panTransform(z, side === 'right' ? offset : { dx: 0, dy: 0 });
  };

  const onFit = () => {
    const host = hostRef.current;
    if (!data) return;
    const w = host?.clientWidth ?? data.width;
    const h = host?.clientHeight ?? data.height;
    setZoom({ scale: fitScale(data.width, data.height, w / 2, h), panX: 0, panY: 0 });
  };

  const onWheel = (e: ReactWheelEvent) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    setZoom((z) => zoomBy(z, e.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP));
  };

  const onKeyDown = (e: ReactKeyboardEvent) => {
    const step = offsetStep(e.key, e.ctrlKey || e.metaKey);
    if (!step) return;
    e.preventDefault();
    setOffset((o) => ({ dx: o.dx + step.dx, dy: o.dy + step.dy }));
  };

  const onMouseDown = (e: ReactMouseEvent) => {
    setPanStart({ x: e.clientX, y: e.clientY, panX: zoom.panX, panY: zoom.panY });
  };
  const onMouseMove = (e: ReactMouseEvent) => {
    if (!panStart) return;
    setZoom((z) => ({
      ...z,
      panX: panStart.panX + (e.clientX - panStart.x),
      panY: panStart.panY + (e.clientY - panStart.y),
    }));
  };

  const visible = blinkSide(blinkMode, tick);
  const blend = blinkMode === 'blend' ? blendPercent(tick) : 1;
  const leftOpacity = blinkMode === 'blink' ? (visible === 'left' ? 1 : 0) : 1;
  const rightOpacity = blinkMode === 'blink'
    ? (visible === 'right' ? 1 : 0)
    : blinkMode === 'blend' ? blend : 1;

  return (
    <div className="aic-picture">
      <div className="aic-picture-toolbar">
        <span className="aic-picture-title">{S.picture.title}: {tab.title.replace('图片比较：', '')}</span>
        <label>
          {S.picture.tolerance}
          <input type="range" min={0} max={64} value={tolerance} onChange={(e) => setTolerance(Number(e.target.value))} />
          <span className="val">{tolerance}</span>
        </label>
        <span className="meta sep">|</span>
        <button onClick={() => setZoom(zoomOut(zoom))} title={S.picture.zoomOutHint}>{S.picture.zoomOut}</button>
        <button onClick={() => setZoom(zoomIn(zoom))} title={S.picture.zoomInHint}>{S.picture.zoomIn}</button>
        <button onClick={onFit} title={S.picture.zoomFitHint}>{S.picture.zoomFit}</button>
        <button onClick={() => setZoom(actualZoom())} title={S.picture.zoomActualHint}>{S.picture.zoomActual}</button>
        <span className="meta">{S.picture.zoomLabel} {Math.round(zoom.scale * 100)}%</span>
        <span className="meta sep">|</span>
        <label>
          {S.picture.blinkMode}
          <select value={blinkMode} onChange={(e) => { setBlinkMode(e.target.value as BlinkMode); setTick(0); }}>
            <option value="off">{S.picture.blinkOff}</option>
            <option value="blink">{S.picture.blinkOn}</option>
            <option value="blend">{S.picture.blend}</option>
          </select>
        </label>
        {blinkMode !== 'off' && (
          <label>
            {S.picture.blinkSpeed}
            <input type="range" min={60} max={1200} step={20} value={blinkMs} onChange={(e) => setBlinkMs(Number(e.target.value))} />
            <span className="val">{blinkMs}ms</span>
          </label>
        )}
        <label className="aic-picture-check">
          <input type="checkbox" checked={autoScale} onChange={(e) => setAutoScale(e.target.checked)} />
          {S.picture.autoScale}
        </label>
        <span className="meta" title={S.picture.offsetHint}>{S.picture.offset} {offset.dx},{offset.dy}</span>
        <button onClick={() => setOffset({ dx: 0, dy: 0 })} title={S.picture.resetOffsetHint}>{S.picture.resetOffset}</button>
      </div>
      <div className="aic-picture-stats">
        {error ? (
          <span className="aic-picture-error">{S.picture.loadFail}: {error}</span>
        ) : (
          <>
            <span>{S.picture.similarity}: <b>{stats}</b></span>
            {data && <span>{S.picture.canvas}: {data.width}×{data.height}</span>}
            {data && <span>{S.picture.blocks}: {data.blocks.length}{data.truncated ? `（${S.picture.truncated}）` : ''}</span>}
            {data && <span>{S.picture.sizes}: {data.leftWidth}×{data.leftHeight} ⇄ {data.rightWidth}×{data.rightHeight}</span>}
          </>
        )}
      </div>
      <div
        className="aic-picture-canvas-host"
        ref={hostRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onWheel={onWheel}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={() => setPanStart(null)}
        onMouseLeave={() => setPanStart(null)}
      >
        <div className="aic-picture-pane">
          <div className="aic-picture-pane-head">{S.picture.paneLeft}</div>
          {isDemo ? (
            <canvas ref={leftCanvasRef} className="aic-picture-layer" style={{ transform: transformFor('left'), opacity: leftOpacity }} />
          ) : (
            <img
              className="aic-picture-layer"
              src={`file://${tab.picture.left}`}
              alt="left"
              style={{ transform: transformFor('left'), opacity: leftOpacity }}
            />
          )}
        </div>
        <div className="aic-picture-pane">
          <div className="aic-picture-pane-head">{S.picture.paneRight}</div>
          {isDemo ? (
            <canvas ref={rightCanvasRef} className="aic-picture-layer" style={{ transform: transformFor('right'), opacity: rightOpacity }} />
          ) : (
            <img
              className="aic-picture-layer"
              src={`file://${tab.picture.right}`}
              alt="right"
              style={{ transform: transformFor('right'), opacity: rightOpacity }}
            />
          )}
          <canvas
            ref={diffCanvasRef}
            className="aic-picture-diff"
            width={data?.width ?? 640}
            height={data?.height ?? 480}
            style={{ transform: transformFor('right'), opacity: blinkMode === 'off' ? 1 : 0 }}
          />
        </div>
      </div>
    </div>
  );
}
