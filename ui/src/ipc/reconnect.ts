import { RpcError } from '../protocol/client';

/**
 * 引擎重连控制器（纯状态机 + 挂接逻辑）：
 * 引擎进程崩溃 → 自动重启一次 → 会话 resync（P2 若提供 session/resync）→
 * 否则 UI 侧全量重放（降级路径：重新 open，视图经 rebindSession 重建缓存）。
 *
 * 横幅状态机（banner）：ok（正常）/ reconnecting（重连中）/ degraded（降级可用）/
 * failed（自动重启失败，可手动重试）。状态与转换如下：
 *
 *   ok --进程退出/写入失败--> reconnecting --重启+握手成功--> resyncing --恢复完成--> ok
 *                                           \--重启失败-------→ failed --手动重试成功--> ok
 *   任意态 --用户切演示引擎--> mock
 */

export type ReconnectPhase = 'ok' | 'reconnecting' | 'resyncing' | 'degraded' | 'failed';

/** 重启尝试上限（任务口径：崩溃后自动重启一次） */
export const MAX_AUTO_RESTARTS = 1;

export interface ReconnectEvent {
  type: 'phase';
  phase: ReconnectPhase;
}

export class ReconnectPolicy {
  private phase: ReconnectPhase = 'ok';
  private restartsThisBurst = 0;
  private readonly listeners = new Set<(phase: ReconnectPhase) => void>();

  constructor(private readonly maxRestarts: number = MAX_AUTO_RESTARTS) {}

  getPhase(): ReconnectPhase {
    return this.phase;
  }

  subscribe(fn: (phase: ReconnectPhase) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private setPhase(p: ReconnectPhase): void {
    this.phase = p;
    for (const fn of this.listeners) fn(p);
  }

  /** 进程意外退出（transport close / 写入失败） */
  onDisconnect(): void {
    if (this.phase === 'reconnecting' || this.phase === 'resyncing') return; // 抖动去重
    this.restartsThisBurst = 0;
    this.setPhase('reconnecting');
  }

  /** 是否还允许自动重启 */
  canAutoRestart(): boolean {
    return this.restartsThisBurst < this.maxRestarts;
  }

  /** 进入一次重启尝试 */
  beginRestart(): void {
    this.restartsThisBurst++;
  }

  /** 重启 + 握手成功，开始恢复会话 */
  onConnected(): void {
    if (this.phase !== 'reconnecting' && this.phase !== 'degraded' && this.phase !== 'failed')
      return;
    this.setPhase('resyncing');
  }

  /** 会话恢复完成 */
  onResynced(): void {
    this.setPhase('ok');
    this.restartsThisBurst = 0;
  }

  /** 重启或恢复失败（横幅给出重试/降级入口） */
  onFailed(): void {
    this.setPhase('failed');
  }

  /** 手动重试（横幅按钮）：清计数重新走 reconnecting */
  manualRetry(): void {
    this.restartsThisBurst = 0;
    this.setPhase('reconnecting');
  }

  /** 切演示引擎等终态 */
  settled(phase: 'ok' | 'degraded' | 'failed'): void {
    this.restartsThisBurst = 0;
    this.setPhase(phase);
  }
}

/** 判断会话恢复请求是否"引擎未实现"（-32601 → 走全量重放降级） */
export function isMethodNotFound(err: unknown): boolean {
  return err instanceof RpcError && err.code === -32601;
}

/**
 * 引擎侧会话种类（= 客户端句柄前缀；文件夹同步复用 dir 会话）。
 * 崩溃恢复必须覆盖全部种类，否则该视图重连后持失效句柄（R3-19）。
 */
export type EngineSessionKind = 'dir' | 'text' | 'merge' | 'merge3' | 'picture' | 'table' | 'hex';

/** 已跟踪会话的描述（重启后按此重建） */
export interface TrackedSession {
  kind: EngineSessionKind;
  /** 对应 open 方法的参数（不含 sessionId；由恢复流程分配新句柄） */
  params: Record<string, unknown>;
}

export interface ResyncOutcome {
  /** 降级重放时的新会话句柄（session/resync 成功时与旧句柄一致） */
  oldSessionId: string;
  newSessionId: string;
  kind: EngineSessionKind;
  mode: 'resync' | 'replay';
}
