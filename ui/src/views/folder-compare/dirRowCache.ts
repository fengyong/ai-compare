import type { DirDeltaParams, DirRow, DirRowsResult, DirStats, ScanProgress, ScanState } from '../../protocol/types';
import { subscribeDirDelta } from '../../ipc/engineProcess';

/**
 * 目录会话的 UI 侧行缓存（docs/03 §6.3："UI 仅窗口 + 计数"）。
 *
 * - 全量模型驻留引擎（arena），这里只持有已拉取的显示行窗口（Map<index, row>）
 * - 窗口化取数：滚动到哪取哪（ensureWindow），带在途去重
 * - delta 通知（dir/delta）增量合并：upsert 按 index 覆盖、removals 删除、stats/进度更新
 * - 视图通过 useSyncExternalStore 订阅 version 变化触发重渲染
 *
 * P0 约束：显示行按 index 稳定（引擎只在尾部追加/原位更新，不做中间插入）；
 * 中间插入的行序重排（过滤/排序变更）属 P2，届时由引擎发全量 rev 重置事件。
 */

export interface DirStatsSnapshot {
  stats: DirStats;
  scanState: ScanState;
  progress: ScanProgress | null;
  total: number;
}

const zeroStats = (): DirStats => ({
  total: 0,
  same: 0,
  diff: 0,
  insignificant: 0,
  orphanLeft: 0,
  orphanRight: 0,
  error: 0,
  unknown: 0,
});

export class DirRowCache {
  readonly sessionId: string;
  private readonly rows = new Map<number, DirRow>();
  private readonly requested = new Set<number>();
  private total = 0;
  private stats: DirStats = zeroStats();
  private scanState: ScanState = 'idle';
  private progress: ScanProgress | null = null;
  private version = 0;
  private readonly listeners = new Set<() => void>();

  constructor(sessionId: string) {
    this.sessionId = sessionId;
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  /** useSyncExternalStore 的快照：仅版本号，行数据经 getRow 读取 */
  getSnapshot = (): number => this.version;

  getRow(index: number): DirRow | undefined {
    return this.rows.get(index);
  }

  getStats(): DirStatsSnapshot {
    return { stats: this.stats, scanState: this.scanState, progress: this.progress, total: this.total };
  }

  /** 合并引擎增量通知（docs/03 §4.2 DirDelta） */
  applyDelta(d: DirDeltaParams): void {
    for (const up of d.upserts) {
      this.rows.set(up.index, up.row);
      if (up.index + 1 > this.total) this.total = up.index + 1;
    }
    for (const idx of d.removals) {
      this.rows.delete(idx);
    }
    if (typeof d.total === 'number' && d.total > this.total) {
      this.total = d.total;
    }
    if (d.stats) this.stats = d.stats;
    if (d.progress !== undefined) this.progress = d.progress;
    if (d.scanState) this.scanState = d.scanState;
    this.bump();
  }

  /**
   * 确保窗口 [offset, offset+count) 内的行已拉取；缺失部分自动合并连续段并去重请求。
   * 返回的 Promise 在全部缺失段取回后 resolve（失败不抛出，保留已取得部分）。
   */
  async ensureWindow(
    offset: number,
    count: number,
    fetch: (offset: number, count: number) => Promise<DirRowsResult>
  ): Promise<void> {
    const limit = offset + count;
    const missing: number[] = [];
    for (let i = offset; i < limit; i++) {
      if (this.total > 0 && i >= this.total) break;
      if (!this.rows.has(i) && !this.requested.has(i)) missing.push(i);
    }
    if (missing.length === 0) return;

    // 合并连续缺失段
    const ranges: Array<{ start: number; end: number }> = [];
    for (const i of missing) {
      const last = ranges[ranges.length - 1];
      if (last && i === last.end) last.end = i + 1;
      else ranges.push({ start: i, end: i + 1 });
    }

    await Promise.all(
      ranges.map(async (r) => {
        for (let i = r.start; i < r.end; i++) this.requested.add(i);
        try {
          const res = await fetch(r.start, r.end - r.start);
          res.rows.forEach((row, i) => {
            const idx = r.start + i;
            this.rows.set(idx, row);
            if (idx + 1 > this.total) this.total = idx + 1;
          });
          if (res.total > this.total) this.total = res.total;
          if (res.scanState !== this.scanState) this.scanState = res.scanState;
          this.bump();
        } catch (err) {
          // 单窗口失败不拖垮视图：保留占位行，下次滚动重试
          console.warn('[dir/rows] 窗口取行失败', r, err);
        } finally {
          for (let i = r.start; i < r.end; i++) this.requested.delete(i);
        }
      })
    );
  }

  private bump(): void {
    this.version++;
    for (const fn of this.listeners) fn();
  }
}

/* 按会话复用的缓存注册表（切换标签不丢状态；会话关闭时由 shell/store 释放） */
const caches = new Map<string, DirRowCache>();

export function getDirCache(sessionId: string): DirRowCache {
  let c = caches.get(sessionId);
  if (!c) {
    c = new DirRowCache(sessionId);
    caches.set(sessionId, c);
  }
  return c;
}

/**
 * 把引擎 dir/delta 持续前送进会话缓存。
 *
 * 订阅挂在缓存（会话级）而非视图组件上：切换标签会卸载视图组件，
 * 若订阅随之解除，隐藏期间的 delta（如子会话关闭的 fileOverride 回传）会丢失。
 * 缓存是引擎模型的投影，只要会话活着就应持续追平（docs/03 §2.1 关键约束 1）。
 * 幂等：同一会话只前送一次；releaseDirCache 时解除。
 */
const forwarders = new Map<string, () => void>();

export function ensureDirCacheAttached(sessionId: string): DirRowCache {
  const cache = getDirCache(sessionId);
  if (!forwarders.has(sessionId)) {
    const off = subscribeDirDelta(sessionId, (d) => cache.applyDelta(d));
    forwarders.set(sessionId, off);
  }
  return cache;
}

export function releaseDirCache(sessionId: string): void {
  caches.delete(sessionId);
  const off = forwarders.get(sessionId);
  if (off) {
    off();
    forwarders.delete(sessionId);
  }
}
