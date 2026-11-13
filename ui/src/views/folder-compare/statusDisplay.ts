import type { DirEntryStatus } from '../../protocol/types';
import { S } from '../../strings';

/**
 * 目录行状态的展示语义（docs/01 附录 C.1 颜色惯例 + FR-DIR-040/041）：
 * 文件名与左侧彩点同色；中间列显示内容比较图标。
 * 状态集合对齐生成协议 DirEntryStatus（P0 子集）。
 */

export type StatusTone = 'same' | 'diff' | 'orphan' | 'unknown';

export interface StatusVisual {
  tone: StatusTone;
  label: string;
  /** 中间列内容图标（仅内容比较相关状态显示） */
  centerGlyph: string;
  centerClass: string;
}

export function statusVisual(status: DirEntryStatus): StatusVisual {
  switch (status) {
    case 'same':
      return { tone: 'same', label: S.folder.status.same, centerGlyph: '=', centerClass: 'same' };
    case 'contentDiff':
      return { tone: 'diff', label: S.folder.status.contentDiff, centerGlyph: '≠', centerClass: 'diff' };
    case 'quickDiff':
      // 快速测试判异（mtime 超容差）且未做内容复核：按"疑似不同"展示
      return { tone: 'diff', label: S.folder.status.quickDiff, centerGlyph: '≈', centerClass: 'diff' };
    case 'leftOnly':
      return { tone: 'orphan', label: S.folder.status.leftOnly, centerGlyph: '', centerClass: '' };
    case 'rightOnly':
      return { tone: 'orphan', label: S.folder.status.rightOnly, centerGlyph: '', centerClass: '' };
    case 'dirSame':
      return { tone: 'same', label: S.folder.status.dirSame, centerGlyph: '', centerClass: '' };
    default:
      return { tone: 'unknown', label: S.folder.status.unknown, centerGlyph: '', centerClass: '' };
  }
}

/** 状态 → CSS 类名（行文字与彩点共用） */
export function toneClass(tone: StatusTone): string {
  return `aic-status-${tone}`;
}

export const statusDotClass: Record<StatusTone, string> = {
  same: 'same',
  diff: 'diff',
  orphan: 'orphan',
  unknown: 'unknown',
};
