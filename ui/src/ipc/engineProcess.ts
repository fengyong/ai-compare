import { RpcClient } from '../protocol/client';
import type {
  DirDeltaParams,
  DirOpenChildParams,
  DirOpenChildResult,
  DirOpenParams,
  DirOpenResult,
  DirRowsParams,
  DirRowsResult,
  InitializeResult,
  SessionCloseParams,
  SessionCloseResult,
  TextCloseParams,
  TextCloseResult,
  TextInlineDiffParams,
  TextInlineDiffResult,
  TextLinesParams,
  TextLinesResult,
  TextOpenParams,
  TextOpenResult,
  TextSectionsParams,
  TextSectionsResult,
} from '../protocol/types';
import { isTauriRuntime } from '../lib/env';
import { MockEngine } from './mockEngine';
import { MockTransport } from './mockTransport';
import { TauriEngineTransport } from './tauriTransport';

/**
 * 引擎连接单例：连接选择（Tauri 真实引擎 / 演示引擎）、状态广播、按会话的 delta 订阅、
 * 以及 P0 方法集的类型化封装。视图层只与该模块交互，不直接触碰 Transport/RpcClient。
 *
 * 优雅降级策略（任务约束：引擎二进制缺失时给提示）：
 * - 浏览器 dev（无 Tauri）：直接使用内置演示引擎（mock），状态栏明确标识"演示模式"；
 * - Tauri 内 engine_start 失败（典型：未构建 engine/target/debug/ai-engine）：进入 degraded，
 *   顶部横幅给出构建指引，可一键重试或切换演示引擎。
 */

export type EngineStatus = 'connecting' | 'ready' | 'degraded' | 'mock';

export interface EngineStateSnapshot {
  status: EngineStatus;
  /** 引擎标识（名称+版本）或降级原因说明 */
  message: string;
}

interface DirDeltaSubscription {
  (delta: DirDeltaParams): void;
}

let client: RpcClient | null = null;
let connectPromise: Promise<RpcClient> | null = null;
let forcedMock = false;
const listeners = new Set<() => void>();
const deltaSubs = new Map<string, Set<DirDeltaSubscription>>();
let snapshot: EngineStateSnapshot = { status: 'connecting', message: '正在连接引擎…' };

function setSnapshot(next: EngineStateSnapshot): void {
  snapshot = next;
  for (const fn of listeners) fn();
}

export function subscribeEngineState(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getEngineState(): EngineStateSnapshot {
  return snapshot;
}

/** 获取（或建立）引擎连接；initialize 能力协商完成后 resolve */
export function getEngineClient(): Promise<RpcClient> {
  if (client) return Promise.resolve(client);
  if (!connectPromise) {
    connectPromise = doConnect().catch((err) => {
      connectPromise = null; // 允许重试
      throw err;
    });
  }
  return connectPromise;
}

async function doConnect(): Promise<RpcClient> {
  setSnapshot({ status: 'connecting', message: '正在连接引擎…' });
  if (isTauriRuntime() && !forcedMock) {
    try {
      const transport = new TauriEngineTransport();
      const c = new RpcClient(transport, {
        onTransportClose: (reason) => {
          client = null;
          setSnapshot({ status: 'degraded', message: `引擎连接断开${reason ? `：${reason}` : ''}` });
        },
      });
      const info: InitializeResult = await c.ready;
      client = c;
      wireNotifications(c);
      setSnapshot({
        status: 'ready',
        message: `${info.engineInfo.name} ${info.engineInfo.version}（协议 v${info.protocolVersion}）`,
      });
      return c;
    } catch (err) {
      // 典型场景：engine/target/debug/ai-engine 未构建。给出可见的降级横幅与指引。
      setSnapshot({
        status: 'degraded',
        message: err instanceof Error ? err.message : String(err),
      });
      throw err;
    }
  }
  return connectMock();
}

function connectMock(): RpcClient {
  const engine = new MockEngine();
  const c = new RpcClient(new MockTransport(engine), {
    onTransportClose: () => {
      client = null;
      setSnapshot({ status: 'connecting', message: '演示引擎已关闭' });
    },
  });
  client = c;
  wireNotifications(c);
  setSnapshot({ status: 'mock', message: '演示引擎（内置模拟数据，非真实 ai-engine）' });
  return c;
}

function wireNotifications(c: RpcClient): void {
  c.on('dir/delta', (params) => {
    const delta = params as DirDeltaParams;
    const subs = deltaSubs.get(delta.sessionId);
    if (subs) {
      for (const fn of subs) fn(delta);
    }
  });
}

/** 顶部横幅"重试"：清空连接并重新协商 */
export function retryEngine(): void {
  forcedMock = false;
  const old = client;
  client = null;
  connectPromise = null;
  void old?.close();
  setSnapshot({ status: 'connecting', message: '正在连接引擎…' });
}

/** 降级为演示引擎（横幅按钮）：重载页面后以 mock 重建所有会话视图 */
export function switchToMockEngine(): void {
  forcedMock = true;
  const old = client;
  client = null;
  connectPromise = null;
  void old?.close();
  // 简单可靠地重建全部视图状态（P0 会话恢复属 P4 范围）
  window.location.reload();
}

// ==================================================================
// P0 方法集类型化封装
// ==================================================================

export async function openFolder(params: DirOpenParams): Promise<DirOpenResult> {
  const c = await getEngineClient();
  return c.request<DirOpenResult>('dir/open', params);
}

export async function getDirRows(params: DirRowsParams): Promise<DirRowsResult> {
  const c = await getEngineClient();
  return c.request<DirRowsResult>('dir/rows', params, 20_000);
}

export async function openChild(params: DirOpenChildParams): Promise<DirOpenChildResult> {
  const c = await getEngineClient();
  return c.request<DirOpenChildResult>('dir/openChild', params);
}

export async function openText(params: TextOpenParams): Promise<TextOpenResult> {
  const c = await getEngineClient();
  return c.request<TextOpenResult>('text/open', params, 60_000);
}

export async function getTextSections(params: TextSectionsParams): Promise<TextSectionsResult> {
  const c = await getEngineClient();
  return c.request<TextSectionsResult>('text/sections', params, 120_000);
}

export async function getTextLines(params: TextLinesParams): Promise<TextLinesResult> {
  const c = await getEngineClient();
  return c.request<TextLinesResult>('text/lines', params, 60_000);
}

export async function getInlineDiff(params: TextInlineDiffParams): Promise<TextInlineDiffResult> {
  const c = await getEngineClient();
  return c.request<TextInlineDiffResult>('text/inlineDiff', params);
}

export async function closeText(params: TextCloseParams): Promise<TextCloseResult> {
  const c = await getEngineClient();
  return c.request<TextCloseResult>('text/close', params);
}

export async function closeSession(params: SessionCloseParams): Promise<SessionCloseResult> {
  const c = await getEngineClient();
  return c.request<SessionCloseResult>('session/close', params);
}

export async function shutdownEngine(): Promise<void> {
  const c = await getEngineClient();
  await c.request('shutdown', null, 5_000);
}

// ==================================================================
// dir/delta 按会话订阅
// ==================================================================

/**
 * 订阅某目录会话的增量推送。注意：调用前应已通过任一 API 建立连接
 * （openFolder 等），否则 sessionId 无法匹配。返回取消订阅函数。
 */
export function subscribeDirDelta(sessionId: string, fn: DirDeltaSubscription): () => void {
  let set = deltaSubs.get(sessionId);
  if (!set) {
    set = new Set();
    deltaSubs.set(sessionId, set);
  }
  set.add(fn);
  return () => {
    set.delete(fn);
    if (set.size === 0) deltaSubs.delete(sessionId);
  };
}
