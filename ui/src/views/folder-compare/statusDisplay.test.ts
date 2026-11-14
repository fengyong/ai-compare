import { describe, expect, it } from 'vitest';
import type { DirEntryStatus } from '../../protocol/types';
import { S } from '../../strings';
import { statusDotClass, statusVisual, toneClass } from './statusDisplay';

/** 协议枚举全集（新增状态必须显式给色，否则落到 default 被当成"未知"灰） */
const ALL: DirEntryStatus[] = [
  'same',
  'quickDiff',
  'contentDiff',
  'similar',
  'leftOnly',
  'rightOnly',
  'dirSame',
  'dirDiff',
  'unknown',
];

describe('statusVisual（docs/01 附录 C.1 颜色惯例）', () => {
  it('除 unknown 外每个状态都有明确语义色，不落到 default', () => {
    for (const s of ALL) {
      const v = statusVisual(s);
      if (s === 'unknown') expect(v.tone).toBe('unknown');
      else expect(v.tone, `${s} 应有明确色`).not.toBe('unknown');
      expect(v.label.length).toBeGreaterThan(0);
    }
  });

  it('similar：基于规则判"仅不重要差异" → 差异红 + ≈ 图标（BC 无独立相似行色）', () => {
    const v = statusVisual('similar');
    expect(v.tone).toBe('diff');
    expect(v.centerGlyph).toBe('≈');
    expect(v.label).toBe(S.folder.status.similar);
  });

  it('dirDiff：聚合出差异的目录 → 差异红（此前落 default 显示为未知灰）', () => {
    const v = statusVisual('dirDiff');
    expect(v.tone).toBe('diff');
    expect(v.label).toBe(S.folder.status.dirDiff);
  });

  it('孤立项紫、相同黑、快速判异红', () => {
    expect(statusVisual('leftOnly').tone).toBe('orphan');
    expect(statusVisual('rightOnly').tone).toBe('orphan');
    expect(statusVisual('same').tone).toBe('same');
    expect(statusVisual('dirSame').tone).toBe('same');
    expect(statusVisual('quickDiff').tone).toBe('diff');
    expect(statusVisual('contentDiff').tone).toBe('diff');
  });

  it('中间列图标只在内容比较相关状态出现（目录/孤立项留空）', () => {
    expect(statusVisual('same').centerGlyph).toBe('=');
    expect(statusVisual('contentDiff').centerGlyph).toBe('≠');
    for (const s of [
      'leftOnly',
      'rightOnly',
      'dirSame',
      'dirDiff',
      'unknown',
    ] as DirEntryStatus[]) {
      expect(statusVisual(s).centerGlyph).toBe('');
    }
  });

  it('tone → CSS 类名与彩点类一致（行文字与左侧彩点同色）', () => {
    for (const s of ALL) {
      const { tone } = statusVisual(s);
      expect(toneClass(tone)).toBe(`aic-status-${tone}`);
      expect(statusDotClass[tone]).toBe(tone);
    }
  });
});
