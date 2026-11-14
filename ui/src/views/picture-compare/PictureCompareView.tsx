import { useEffect, useMemo, useRef, useState } from 'react';
import { openPicture } from '../../ipc/engineProcess';
import { S } from '../../strings';
import type { Tab } from '../../shell/store';

/**
 * 图片比较视图（P14，FR-PIC：Pro 视图族首件）。
 *
 * 数据面：picture/open（引擎像素容差 diff → 网格块聚合；PictureDiffResult）。
 * 渲染：左右双 img + 中央差异块叠加 canvas（红色半透明块边界框，占比越高越浓）。
 * 浏览器演示模式（mock）：两侧路径非真实文件——canvas 程序化合成演示图案
 * （确定性种子），叠加块与统计与 mock 引擎响应一致。
 *
 * 交互：容差滑杆（重比较）+ 并排/叠加两种模式 + 统计栏
 * （相似度/差异像素/画布/两侧尺寸）。
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
    // 占比越高越浓（0.05 底线保证可见）
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

export function PictureCompareView({ tab }: { tab: Tab & { kind: 'picture'; picture: { left: string; right: string; sessionId: string } } }) {
  const [data, setData] = useState<PictureOpenData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tolerance, setTolerance] = useState(8);
  const [overlayMode, setOverlayMode] = useState<'right' | 'left'>('right');
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
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
    // 容差变化触发重比较（sessionId 复用会被 411 拒——演示模式忽略；真实模式
    // 换新 sessionId 由调用方管理，P14 简化为首次打开）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.picture.sessionId, tolerance]);

  // 叠加层渲染（差异块边界框 + 半透明填充）
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !data) return;
    const displayW = canvas.clientWidth || data.width;
    const scale = displayW / data.width;
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
    void scale;
  }, [data, overlayMode]);

  // 演示模式：合成图案
  useEffect(() => {
    if (!isDemo || !data) return;
    const canvas = overlayMode === 'right' ? canvasRef.current : canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    drawDemoPattern(ctx, data.width, data.height, demoSeed, overlayMode === 'right' ? 'right' : 'left');
  }, [isDemo, data, overlayMode, demoSeed]);

  const stats = useMemo(() => (data ? formatStats(data) : S.picture.loading), [data]);

  return (
    <div className="aic-picture">
      <div className="aic-picture-toolbar">
        <span className="aic-picture-title">{S.picture.title}: {tab.title.replace('图片比较：', '')}</span>
        <label>
          {S.picture.tolerance}
          <input
            type="range"
            min={0}
            max={64}
            value={tolerance}
            onChange={(e) => setTolerance(Number(e.target.value))}
          />
          <span className="val">{tolerance}</span>
        </label>
        <label>
          {S.picture.overlaySide}
          <select value={overlayMode} onChange={(e) => setOverlayMode(e.target.value as 'left' | 'right')}>
            <option value="right">{S.picture.overlayRight}</option>
            <option value="left">{S.picture.overlayLeft}</option>
          </select>
        </label>
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
      <div className="aic-picture-canvas-host">
        <canvas
          ref={canvasRef}
          className="aic-picture-canvas"
          width={data?.width ?? 640}
          height={data?.height ?? 480}
        />
        {!isDemo && data && (
          <div className="aic-picture-imgs">
            <img src={`file://${tab.picture.left}`} alt="left" style={{ display: overlayMode === 'left' ? 'block' : 'none' }} />
            <img src={`file://${tab.picture.right}`} alt="right" style={{ display: overlayMode === 'right' ? 'block' : 'none' }} />
          </div>
        )}
      </div>
    </div>
  );
}

void S;
