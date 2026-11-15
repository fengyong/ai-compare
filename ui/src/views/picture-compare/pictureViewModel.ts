/**
 * 图片比较视图模型（docs/01 §3.8 FR-PIX-002/003/004）。
 * 纯函数：缩放状态、自动缩放几何、闪烁/混合相位、差异偏移。
 */

export interface ZoomState {
  /** 1 = 实际像素；>1 放大；<1 缩小 */
  scale: number;
  panX: number;
  panY: number;
}

export const MIN_ZOOM = 0.05;
export const MAX_ZOOM = 16;
export const ZOOM_STEP = 1.25;

export function clampZoom(scale: number): number {
  if (!Number.isFinite(scale) || scale <= 0) return 1;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, scale));
}

export function zoomBy(state: ZoomState, factor: number): ZoomState {
  return { ...state, scale: clampZoom(state.scale * factor) };
}

export function zoomIn(state: ZoomState): ZoomState {
  return zoomBy(state, ZOOM_STEP);
}

export function zoomOut(state: ZoomState): ZoomState {
  return zoomBy(state, 1 / ZOOM_STEP);
}

/** 缩放到适合：视图区完全容纳画布（留 8px 边距） */
export function fitScale(canvasW: number, canvasH: number, viewW: number, viewH: number): number {
  if (canvasW <= 0 || canvasH <= 0 || viewW <= 0 || viewH <= 0) return 1;
  return clampZoom(Math.min((viewW - 8) / canvasW, (viewH - 8) / canvasH));
}

export function actualZoom(): ZoomState {
  return { scale: 1, panX: 0, panY: 0 };
}

/**
 * FR-PIX-003 自动缩放：放大较小图像匹配较大图像比例。
 * 以两侧最大宽/高为目标画布，分别给出各自的等比缩放系数。
 */
export function autoScaleGeometry(
  left: { w: number; h: number },
  right: { w: number; h: number },
): { left: number; right: number } {
  if (left.w <= 0 || left.h <= 0 || right.w <= 0 || right.h <= 0) return { left: 1, right: 1 };
  // 以两侧最大宽度作为统一目标画布：两图缩放后宽度一致，高度按各自
  // 比例保持（避免分别取 max 造成宽高比失真，FR-PIX-003「匹配较大图像比例」）。
  const maxW = Math.max(left.w, right.w);
  return { left: maxW / left.w, right: maxW / right.w };
}

/** 平移量钳制：避免把画布拖出视图（允许小幅留白） */
export function clampPan(pan: number, scale: number, canvasSize: number, viewSize: number): number {
  const scaled = canvasSize * scale;
  const slack = Math.max(0, (scaled - viewSize) / 2);
  return Math.min(slack + 40, Math.max(-slack - 40, pan));
}

export type BlinkMode = 'off' | 'blink' | 'blend';

/**
 * 闪烁/混合相位：
 * - off：始终显示差异叠加（右侧）；
 * - blink：偶数拍显示左图、奇数拍显示右图（BC 的淡入淡出/blink 动态版）；
 * - blend：三角波 0→1→0 的混合百分比（FR-PIX-002 混合模式）。
 */
export function blinkSide(mode: BlinkMode, tick: number): 'left' | 'right' | 'diff' {
  if (mode === 'blink') return tick % 2 === 0 ? 'left' : 'right';
  return 'diff';
}

/** 三角波混合百分比（0 = 左侧主导，1 = 右侧主导；周期 2*steps） */
export function blendPercent(tick: number, steps = 10): number {
  const period = Math.max(2, steps * 2);
  const phase = ((tick % period) + period) % period;
  return phase <= steps ? phase / steps : (period - phase) / steps;
}

/** FR-PIX-003 差异偏移：箭头键 1px，Ctrl+箭头 10px */
export function offsetStep(key: string, ctrl: boolean): { dx: number; dy: number } | null {
  const step = ctrl ? 10 : 1;
  switch (key) {
    case 'ArrowLeft':
      return { dx: -step, dy: 0 };
    case 'ArrowRight':
      return { dx: step, dy: 0 };
    case 'ArrowUp':
      return { dx: 0, dy: -step };
    case 'ArrowDown':
      return { dx: 0, dy: step };
    default:
      return null;
  }
}

/** 两窗格共用的变换串（缩放同步：同一 ZoomState 驱动所有 pane） */
export function panTransform(
  zoom: ZoomState,
  offset: { dx: number; dy: number } = { dx: 0, dy: 0 },
): string {
  return `translate(${zoom.panX + offset.dx}px, ${zoom.panY + offset.dy}px) scale(${zoom.scale})`;
}
