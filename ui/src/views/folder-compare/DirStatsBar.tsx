import { formatCount } from '../../lib/env';
import { S } from '../../strings';
import type { DirStatsSnapshot } from './dirRowCache';

/**
 * 目录比较统计栏（FR-DIR-072 会话信息的 P0 版）：各状态计数 chips + 扫描进度。
 * 同时充当图例（颜色 = 语义）。计数对齐生成协议 DirStats。
 */
export function DirStatsBar({ snapshot }: { snapshot: DirStatsSnapshot }) {
  const { stats, scanState, progress, scanErrors } = snapshot;
  const percent =
    progress && progress.total ? Math.min(100, Math.round((progress.scanned / progress.total) * 100)) : null;
  const errorHint =
    scanErrors.length > 0
      ? `${S.folder.scanErrorHint}\n${scanErrors.map((e) => `${e.path}：${e.message}`).join('\n')}`
      : undefined;

  return (
    <>
      <div className="aic-stats">
        <span className="aic-stat-chip">
          <i className="aic-dot same" />
          <b>{formatCount(stats.same)}</b> {S.folder.stats.same}
        </span>
        <span className="aic-stat-chip">
          <i className="aic-dot diff" />
          <b>{formatCount(stats.contentDiff)}</b> {S.folder.stats.contentDiff}
        </span>
        <span className="aic-stat-chip">
          <i className="aic-dot diff" />
          <b>{formatCount(stats.quickDiff)}</b> {S.folder.stats.quickDiff}
        </span>
        <span className="aic-stat-chip">
          <i className="aic-dot orphan" />
          <b>{formatCount(stats.leftOnly)}</b> {S.folder.stats.leftOnly}
        </span>
        <span className="aic-stat-chip">
          <i className="aic-dot orphan" />
          <b>{formatCount(stats.rightOnly)}</b> {S.folder.stats.rightOnly}
        </span>
        <span className="aic-stat-chip">
          <i className="aic-dot same" />
          <b>{formatCount(stats.dirSame)}</b> {S.folder.stats.dirSame}
        </span>
      </div>
      {scanErrors.length > 0 && (
        <span className="aic-stat-chip" style={{ color: 'var(--aic-st-error)' }} title={errorHint}>
          <b>{formatCount(scanErrors.length)}</b> {S.folder.scanErrorCount}
        </span>
      )}
      <span className="spacer" />
      {scanState === 'running' && (
        <span className="aic-progress">
          {S.folder.scanning} {formatCount(progress?.scanned ?? 0)} /{' '}
          {progress?.total !== null && progress?.total !== undefined ? formatCount(progress.total) : '?'}
          <span className="bar">
            <i style={{ width: `${percent ?? 0}%` }} />
          </span>
        </span>
      )}
      {scanState === 'completed' && <span style={{ color: 'var(--aic-text-dim)' }}>{S.folder.scanDone}</span>}
    </>
  );
}
