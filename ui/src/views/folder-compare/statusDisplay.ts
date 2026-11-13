import type { DirCellStatus } from '../../protocol/types';
import { S } from '../../strings';

/**
 * 目录行状态的展示语义（docs/01 附录 C.1 颜色惯例 + FR-DIR-040/041）：
 * 文件名与左侧彩点同色；中间列显示内容比较图标。
 */

export type StatusTone = 'same' | 'diff' | 'orphan' | 'older' | 'error' | 'unknown' | 'unscanned';

export interface StatusVisual {
  tone: StatusTone;
  label: string;
  /** 中间列内容图标（仅内容比较相关状态显示） */
  centerGlyph: string;
  centerClass: string;
}

export function statusVisual(status: DirCellStatus): StatusVisual {
  switch (status) {
    case 'same':
      return { tone: 'same', label: S.folder.status.same, centerGlyph: '=', centerClass: 'same' };
    case 'contentDiff':
      return { tone: 'diff', label: S.folder.status.contentDiff, centerGlyph: '≠', centerClass: 'diff' };
    case 'ruleDiff':
      return { tone: 'diff', label: S.folder.status.ruleDiff, centerGlyph: '≠', centerClass: 'diff' };
    case 'insignificantDiff':
      return { tone: 'diff', label: S.folder.status.insignificantDiff, centerGlyph: '≈', centerClass: 'insig' };
    case 'newerLeft':
      return { tone: 'diff', label: S.folder.status.newerLeft, centerGlyph: '', centerClass: 'diff' };
    case 'newerRight':
      return { tone: 'diff', label: S.folder.status.newerRight, centerGlyph: '', centerClass: 'diff' };
    case 'olderLeft':
      return { tone: 'older', label: S.folder.status.olderLeft, centerGlyph: '', centerClass: '' };
    case 'olderRight':
      return { tone: 'older', label: S.folder.status.olderRight, centerGlyph: '', centerClass: '' };
    case 'orphanLeft':
      return { tone: 'orphan', label: S.folder.status.orphanLeft, centerGlyph: '', centerClass: '' };
    case 'orphanRight':
      return { tone: 'orphan', label: S.folder.status.orphanRight, centerGlyph: '', centerClass: '' };
    case 'error':
      return { tone: 'error', label: S.folder.status.error, centerGlyph: '!', centerClass: 'error' };
    case 'scanning':
      return { tone: 'unscanned', label: S.folder.status.scanning, centerGlyph: '', centerClass: '' };
    case 'unscanned':
      return { tone: 'unscanned', label: S.folder.status.unscanned, centerGlyph: '', centerClass: '' };
    default:
      return { tone: 'unknown', label: S.folder.status.unknown, centerGlyph: '', centerClass: '' };
  }
}

/** 行的最终生效状态：子会话回传覆盖优先（fileOverride，docs/03 §5.1） */
export function effectiveStatus(row: { status: DirCellStatus; fileOverride?: DirCellStatus | null }): DirCellStatus {
  return row.fileOverride ?? row.status;
}

/** 状态 → CSS 类名（行文字与彩点共用） */
export function toneClass(tone: StatusTone): string {
  return `aic-status-${tone}`;
}

export const statusDotClass: Record<StatusTone, string> = {
  same: 'same',
  diff: 'diff',
  orphan: 'orphan',
  older: 'older',
  error: 'error',
  unknown: 'unknown',
  unscanned: 'unscanned',
};
