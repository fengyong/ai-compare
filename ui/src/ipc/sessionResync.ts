import type { EngineSessionKind, ResyncOutcome, TrackedSession } from './reconnect';

/**
 * 引擎崩溃重连后的会话恢复（R3-19）。
 *
 * 逐会话先试 `session/resync`（引擎保留状态，句柄不变）；不支持/会话已不存在则
 * 降级：分配新句柄并广播 replay，由视图全量重放。**新句柄必须重新登记**，
 * 否则第二次崩溃时该会话不再被恢复。
 *
 * 恢复必须覆盖全部 EngineSessionKind：此前只登记 dir/text，
 * picture/table/hex/merge/merge3 崩溃后持失效句柄（后续请求全 410/411）。
 */

export interface ResyncIO {
  /** 引擎侧 session/resync；抛错 = 不支持或会话不存在 → 降级重放 */
  resync(sessionId: string): Promise<void>;
  /** 降级重放时分配新句柄 */
  newSessionId(kind: EngineSessionKind): string;
  /** 广播恢复结果（视图据此 rebindSession） */
  emit(outcome: ResyncOutcome): void;
}

export async function resyncSessions(
  tracked: Map<string, TrackedSession>,
  io: ResyncIO,
): Promise<void> {
  for (const [sessionId, t] of [...tracked.entries()]) {
    try {
      await io.resync(sessionId);
      io.emit({
        oldSessionId: sessionId,
        newSessionId: sessionId,
        kind: t.kind,
        mode: 'resync',
      });
    } catch {
      const newId = io.newSessionId(t.kind);
      tracked.delete(sessionId);
      tracked.set(newId, t);
      io.emit({
        oldSessionId: sessionId,
        newSessionId: newId,
        kind: t.kind,
        mode: 'replay',
      });
    }
  }
}
