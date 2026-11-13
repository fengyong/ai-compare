import { useEffect, useRef } from 'react';
import type { AlignedRow } from './alignment';
import {
  OVERVIEW_COLORS,
  buildOverviewSegments,
  rowAtFraction,
  type Importance,
} from './overview';
import type { FoldRange } from './folding';

/**
 * 概览条（FR-TXT-030 / docs/03 §3.4.3 缩略图总览条）：
 * mini-map 式差异色带（每行一像素段，游程编码）+ 视口矩形 + 当前方块高亮。
 * 点击/拖动跳转对应行；与滚动同步（父级把滚动位置换算为视口行号传入）。
 */

export interface OverviewBarProps {
  rows: AlignedRow[];
  importance: Map<number, Importance>;
  /** 折叠隐藏区间（色带上以更暗的覆盖层示意） */
  folds: FoldRange[];
  /** 当前差异 section 的对齐行区间（高亮框；null 隐藏） */
  currentRange: { fromRow: number; toRow: number } | null;
  /** 视口行区间（含端点；null 隐藏视口矩形） */
  viewport: { topRow: number; bottomRow: number } | null;
  /** 跳转请求（对齐行号） */
  onJump(row: number): void;
}

export function OverviewBar({ rows, importance, folds, currentRange, viewport, onJump }: OverviewBarProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const jumpRef = useRef(onJump);
  jumpRef.current = onJump;

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;

    const draw = (): void => {
      const cssH = wrap.clientHeight;
      const cssW = wrap.clientWidth;
      if (cssH <= 0 || cssW <= 0) return;
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.round(cssW * dpr);
      canvas.height = Math.round(cssH * dpr);
      canvas.style.width = `${cssW}px`;
      canvas.style.height = `${cssH}px`;
      const ctx = canvas.getContext('2d');
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, cssW, cssH);

      const total = rows.length;
      const scale = total > 0 ? cssH / total : 0;
      // 底色
      ctx.fillStyle = 'var(--aic-editor-bg)';
      ctx.fillRect(0, 0, cssW, cssH);

      if (total > 0) {
        const segs = buildOverviewSegments(rows, importance);
        for (const seg of segs) {
          const y = seg.startRow * scale;
          const h = Math.max(1, seg.rows * scale);
          ctx.fillStyle = OVERVIEW_COLORS[seg.color];
          ctx.fillRect(2, y, cssW - 4, h);
        }
        // 折叠区覆盖（暗色斜纹示意"此处隐藏了等值区"）
        ctx.fillStyle = 'rgba(0, 0, 0, 0.35)';
        for (const f of folds) {
          const y = f.fromRow * scale;
          const h = Math.max(1, (f.toRow - f.fromRow + 1) * scale);
          ctx.fillRect(2, y, cssW - 4, h);
        }
      }

      // 当前方块
      if (currentRange && total > 0) {
        const y = currentRange.fromRow * scale;
        const h = Math.max(2, (currentRange.toRow - currentRange.fromRow + 1) * scale);
        ctx.strokeStyle = 'var(--aic-accent)';
        ctx.lineWidth = 1.5;
        ctx.strokeRect(1, y - 0.5, cssW - 2, h + 1);
      }

      // 视口矩形
      if (viewport && total > 0) {
        const top = Math.max(0, viewport.topRow) * scale;
        const bottom = Math.min(total, viewport.bottomRow + 1) * scale;
        const h = Math.max(6, bottom - top);
        ctx.fillStyle = 'rgba(255, 255, 255, 0.14)';
        ctx.fillRect(0, top, cssW, h);
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.35)';
        ctx.lineWidth = 1;
        ctx.strokeRect(0.5, top + 0.5, cssW - 1, Math.max(5, h - 1));
      }
    };

    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(wrap);
    return () => ro.disconnect();
  }, [rows, importance, folds, currentRange, viewport]);

  const jumpFromEvent = (ev: React.MouseEvent<HTMLDivElement>): void => {
    const rect = ev.currentTarget.getBoundingClientRect();
    const frac = (ev.clientY - rect.top) / rect.height;
    jumpRef.current(rowAtFraction(rows.length, frac));
  };

  return (
    <div
      className="aic-overview"
      ref={wrapRef}
      onMouseDown={jumpFromEvent}
      onMouseMove={(ev) => {
        if (ev.buttons & 1) jumpFromEvent(ev);
      }}
      title="概览条：色带=差异行（红=重要/蓝=不重要/暗=被折叠），白框=当前视口；点击跳转"
    >
      <canvas ref={canvasRef} />
    </div>
  );
}
