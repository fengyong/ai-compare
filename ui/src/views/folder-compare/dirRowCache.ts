import type { DirDelta, DirRow, DirScanIssue, DirStats, ScanProgress, ScanState } from '../../protocol/types';
import { subscribeDirDelta } from '../../ipc/engineProcess';

/**
 * 目录会话的 UI 侧行缓存（docs/03 §6.3："UI 仅窗口 + 计数"）。
 *
 * P0 数据面：引擎 dir/open 后把全部行按批经 dir/delta 推送（ADR-003 批量），
 * UI 侧按到达顺序维护行数组，upsert 按行 id（相对路径）幂等更新/追加。
 * 视图通过 useSyncExternalStore 订阅 version 变化触发重渲染。
 *
 * P1+ 演进：窗口化拉取（dir/rows）、过滤/排序引起的行序重排（全量 rev 重置）
 * 由引擎侧后续方法承接，届时仅改本类的填充逻辑。
 */

export interface DirStatsSnapshot {
  stats: DirStats;
  scanState: ScanState;
  progress: ScanProgress | null;
  total: number;
  /** 扫描失败的路径（R3-09：读不到 ≠ 相同，必须让用户看见） */
  scanErrors: readonly DirScanIssue[];
}

const zeroStats = (): DirStats => ({
  total: 0,
  same: 0,
  quickDiff: 0,
  contentDiff: 0,
  leftOnly: 0,
  rightOnly: 0,
  dirSame: 0,
  dirDiff: 0,
  similar: 0,
});

export class DirRowCache {
  readonly sessionId: string;
  private readonly rows: DirRow[] = [];
  private readonly indexOfId = new Map<string, number>();
  /**
   * P6b：空格标记集（BC 键盘惯例）。挂在会话缓存层——切换标签/重渲染不丢，
   * 以行 id（rel 路径）为键，与排序/过滤正交（表头排序不冲突）。
   */
  private readonly marks = new Set<string>();
  private stats: DirStats = zeroStats();
  private readonly scanErrors = new Map<string, string>();
  private scanState: ScanState = 'running';
  private progress: ScanProgress | null = null;
  private version = 0;
  private readonly listeners = new Set<() => void>();
  private openPromise: Promise<void> | null = null;

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
    return this.rows[index];
  }

  getRows(): readonly DirRow[] {
    return this.rows;
  }

  getStats(): DirStatsSnapshot {
    return {
      stats: this.stats,
      scanState: this.scanState,
      progress: this.progress,
      total: this.rows.length,
      scanErrors: [...this.scanErrors].map(([path, message]) => ({ path, message })),
    };
  }

  // ---- P6b：标记集（会话缓存层持久；变更推版本号触发视图重渲染） ----

  getMarks(): ReadonlySet<string> {
    return this.marks;
  }

  isMarked(id: string): boolean {
    return this.marks.has(id);
  }

  getMarkCount(): number {
    return this.marks.size;
  }

  /** 切换单行标记，返回切换后的状态 */
  toggleMark(id: string): boolean {
    if (this.marks.has(id)) this.marks.delete(id);
    else this.marks.add(id);
    this.bump();
    return this.marks.has(id);
  }

  /** 批量设置标记（选中行整批打标/取消） */
  setMarks(ids: readonly string[], value: boolean): void {
    let changed = false;
    for (const id of ids) {
      if (value && !this.marks.has(id)) {
        this.marks.add(id);
        changed = true;
      } else if (!value && this.marks.has(id)) {
        this.marks.delete(id);
        changed = true;
      }
    }
    if (changed) this.bump();
  }

  /** dir/open 应答到达：行数终值与扫描态（delta 批次推完后 rev 与之对齐） */
  applyOpenResult(rowCount: number, scanState: ScanState): void {
    this.scanState = scanState;
    if (rowCount > this.rows.length) this.bump();
  }

  /** 合并引擎增量通知（docs/03 §4.2 DirDelta；upsert 按行 id 幂等） */
  applyDelta(d: DirDelta): void {
    for (const row of d.upserts) {
      const idx = this.indexOfId.get(row.id);
      if (idx === undefined) {
        this.indexOfId.set(row.id, this.rows.length);
        this.rows.push(row);
      } else {
        this.rows[idx] = row;
      }
    }
    for (const id of d.removals) {
      const idx = this.indexOfId.get(id);
      if (idx !== undefined) {
        this.indexOfId.delete(id); // 必须同步清 id 索引：否则后续同 id upsert 会被误判为更新，覆盖别的行
        this.rows.splice(idx, 1);
        this.reindex(idx);
        this.marks.delete(id); // 行已消失，标记同步清除（防悬挂 id：仅显示标记过滤出空行）
      }
    }
    // 引擎对 0 值计数器省略键（dirDiff/similar 的 skip_serializing_if），
    // 故用零值兜底合并，避免 UI 读到 undefined
    this.stats = d.stats ? { ...zeroStats(), ...d.stats } : this.stats;
    this.progress = d.progress ?? null;
    // 首批携带当前全量错误（引擎每条 delta 链路都在 batch 0 回传）：先清后收，
    // 重扫后已恢复可读的路径不会留下陈旧错误；后续批次为空，不得清掉首批事实。
    if (d.batch?.index === 0) this.scanErrors.clear();
    for (const e of d.scanErrors ?? []) this.scanErrors.set(e.path, e.message);
    this.bump();
  }

  /**
   * 会话打开编排（幂等，StrictMode 双调用安全）：只发起一次 dir/open。
   * delta 前送由 ensureDirCacheAttached 在 open 之前挂好，无应答/通知竞态。
   */
  ensureOpened(open: () => Promise<{ rowCount: number; scanState: ScanState }>): Promise<void> {
    this.openPromise ??= open()
      .then((r) => {
        this.applyOpenResult(r.rowCount, r.scanState);
      })
      .catch((err: unknown) => {
        this.openPromise = null; // 失败允许重试
        throw err;
      });
    return this.openPromise;
  }

  private reindex(from: number): void {
    for (let i = from; i < this.rows.length; i++) {
      this.indexOfId.set(this.rows[i].id, i);
    }
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
 * 取会话缓存并保证 dir/delta 持续前送（幂等：同一会话只前送一次）。
 *
 * 订阅挂在缓存（会话级）而非视图组件上：切换标签会卸载视图组件，
 * 若订阅随之解除，隐藏期间的 delta 会丢失。缓存是引擎模型的投影，
 * 只要会话活着就应持续追平（docs/03 §2.1 关键约束 1）。
 * releaseDirCache 时解除。
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
