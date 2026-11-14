import { describe, expect, it } from 'vitest';
import {
  MAX_ZOOM,
  MIN_ZOOM,
  ZOOM_STEP,
  actualZoom,
  autoScaleGeometry,
  blendPercent,
  blinkSide,
  clampPan,
  clampZoom,
  fitScale,
  offsetStep,
  panTransform,
  zoomBy,
  zoomIn,
  zoomOut,
} from './pictureViewModel';

describe('图片缩放（FR-PIX-004）', () => {
  it('钳制非法/越界缩放到 [0.05, 16]', () => {
    expect(clampZoom(0)).toBe(1);
    expect(clampZoom(-2)).toBe(1);
    expect(clampZoom(Number.NaN)).toBe(1);
    expect(clampZoom(0.001)).toBe(MIN_ZOOM);
    expect(clampZoom(99)).toBe(MAX_ZOOM);
  });

  it('放大/缩小按 1.25 步长且互为逆操作', () => {
    const z = actualZoom();
    expect(zoomIn(z).scale).toBeCloseTo(ZOOM_STEP);
    expect(zoomOut(zoomIn(z)).scale).toBeCloseTo(1);
    expect(zoomBy(z, 0).scale).toBe(1); // 非法因子回退 1，与 clampZoom(0) 一致
  });

  it('缩放到适合视图（留边距）与实际像素', () => {
    expect(actualZoom()).toEqual({ scale: 1, panX: 0, panY: 0 });
    expect(fitScale(640, 480, 320, 240)).toBeCloseTo((320 - 8) / 640);
    expect(fitScale(0, 0, 320, 240)).toBe(1);
    expect(fitScale(640, 480, 5000, 5000)).toBeLessThan(MAX_ZOOM);
  });

  it('平移钳制与同步变换串', () => {
    expect(clampPan(9999, 1, 100, 200)).toBe(40);
    expect(clampPan(-9999, 1, 100, 200)).toBe(-40);
    expect(panTransform({ scale: 2, panX: 10, panY: -5 })).toBe('translate(10px, -5px) scale(2)');
    expect(panTransform({ scale: 1, panX: 0, panY: 0 }, { dx: 3, dy: 4 })).toBe('translate(3px, 4px) scale(1)');
  });
});

describe('几何对齐（FR-PIX-003）', () => {
  it('自动缩放：较小图放大匹配较大图比例，较大图保持 1', () => {
    const g = autoScaleGeometry({ w: 100, h: 100 }, { w: 200, h: 200 });
    expect(g.left).toBe(2);
    expect(g.right).toBe(1);
    expect(g.left * 100).toBe(g.right * 200);
  });

  it('非等比尺寸取宽高较大因子；非法尺寸回退 1', () => {
    const g = autoScaleGeometry({ w: 100, h: 50 }, { w: 200, h: 200 });
    expect(g.left).toBe(4);
    expect(g.right).toBe(1);
    expect(autoScaleGeometry({ w: 0, h: 0 }, { w: 1, h: 1 })).toEqual({ left: 1, right: 1 });
  });
});

describe('闪烁 / 混合 / 差异偏移（FR-PIX-002/003）', () => {
  it('off 显示差异叠加；blink 按拍切换左右', () => {
    expect(blinkSide('off', 5)).toBe('diff');
    expect(blinkSide('blend', 5)).toBe('diff');
    expect(blinkSide('blink', 0)).toBe('left');
    expect(blinkSide('blink', 1)).toBe('right');
    expect(blinkSide('blink', 8)).toBe('left');
  });

  it('混合百分比为三角波 0→1→0', () => {
    expect(blendPercent(0)).toBe(0);
    expect(blendPercent(10)).toBe(1);
    expect(blendPercent(20)).toBe(0);
    expect(blendPercent(5)).toBe(0.5);
    expect(blendPercent(-1)).toBeGreaterThan(0);
  });

  it('箭头键 1px、Ctrl+箭头 10px；其它键忽略', () => {
    expect(offsetStep('ArrowLeft', false)).toEqual({ dx: -1, dy: 0 });
    expect(offsetStep('ArrowRight', true)).toEqual({ dx: 10, dy: 0 });
    expect(offsetStep('ArrowUp', false)).toEqual({ dx: 0, dy: -1 });
    expect(offsetStep('ArrowDown', true)).toEqual({ dx: 0, dy: 10 });
    expect(offsetStep('a', false)).toBeNull();
  });
});
