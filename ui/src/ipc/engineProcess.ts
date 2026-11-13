import { invoke } from '@tauri-apps/api/core';
import { RpcClient, RpcError } from '../protocol/client';
import type {
  DirDelta,
  DirOpenParams,
  DirOpenResult,
  InitializeResult,
  SessionCloseParams,
  SessionCloseResult,
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
 * - Tauri 内 engine_start 失败（典型：未构建 target/debug/ai-engine）：进入 degraded，
 *   顶部横幅给出构建指引，可一键重试或切换演示引擎。
 *
 * 会话寻址：sessionId 由客户端分配（docs/03 §2.3.2 / ai-protocol DirOpenParams），
 * newSessionId() 统一生成。
 */

export type EngineStatus = 'connecting' | 'ready' | 'degraded' | 'mock';

export interface EngineStateSnapshot {
  status: EngineStatus;
  /** 引擎标识（名称+版本）或降级原因说明 */
  message: string;
}

interface DirDeltaSubscription {
  (delta: DirDelta): void;
}

let client: RpcClient | null = null;
let connectPromise: Promise<RpcClient> | null = null;
let forcedMock = false;
let sessionSeq = 0;
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

/** 生成引擎侧会话句柄（客户端分配；前缀便于日志排查） */
export function newSessionId(kind: 'dir' | 'text'): string {
  const rand =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID().slice(0, 8)
      : String(Math.floor(Math.random() * 0xffffffff));
  return `${kind}-${++sessionSeq}-${rand}`;
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
        message: `${info.engine.name} ${info.engine.version}（协议 v${info.protocolVersion}）`,
      });
      return c;
    } catch (err) {
      // 典型场景：target/debug/ai-engine 未构建。给出可见的降级横幅与指引。
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
    const delta = params as DirDelta;
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
// P0 方法集类型化封装（与 ai-engine handlers.rs 的方法集一致）
// ==================================================================

export async function openFolder(params: DirOpenParams): Promise<DirOpenResult> {
  const c = await getEngineClient();
  return c.request<DirOpenResult>('dir/open', params);
}

export async function openText(params: TextOpenParams): Promise<TextOpenResult> {
  const c = await getEngineClient();
  return c.request<TextOpenResult>('text/open', params, 60_000);
}

export async function getTextSections(params: TextSectionsParams): Promise<TextSectionsResult> {
  const c = await getEngineClient();
  return c.request<TextSectionsResult>('text/sections', params, 120_000);
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
// 行内容获取：text/lines（mock 扩展）→ Tauri 兜底通道
// ==================================================================

/**
 * 拉取一侧的行内容（分页循环拉全）。
 *
 * 引擎 P0 未开放 text/lines（docs/03 §2.3.2 大内容分页，P1 计划）：
 * - 演示引擎实现了 text/lines（浏览器 dev 的唯一内容来源）；
 * - 真实引擎返回 -32601 时，走 Tauri 壳的 P0 兜底命令 read_text_file
 *   直接读本地文件（P1 引擎开放 text/lines 后移除该分支）。
 */
export async function loadSideLines(
  sessionId: string,
  side: 'left' | 'right',
  lineCount: number,
  fallbackPath: string
): Promise<string[]> {
  const c = await getEngineClient();
  const chunk = 4000;
  try {
    const lines: string[] = [];
    let start = 0;
    // 防御性上限：协议异常（done 永不置位）时避免死循环
    let guard = 0;
    while (start < lineCount && guard++ < 10_000) {
      const res = await c.request<TextLinesResult>(
        'text/lines',
        { sessionId, side, start, count: chunk } satisfies TextLinesParams,
        60_000
      );
      lines.push(...res.lines);
      if (res.done || res.lines.length === 0) break;
      start += res.lines.length;
    }
    return lines;
  } catch (err) {
    if (err instanceof RpcError && err.code === -32601) {
      if (!isTauriRuntime()) throw err;
      return readLinesViaTauri(fallbackPath);
    }
    throw err;
  }
}

async function readLinesViaTauri(path: string): Promise<string[]> {
  const res = await invoke<{ text: string }>('read_text_file', { path });
  return splitLines(res.text);
}

/** 与引擎 LineTable::from_text 同口径：行文本不含行尾符；结尾换行不产生空行 */
export function splitLines(text: string): string[] {
  if (text === '') return [];
  const parts = text.split('\n');
  if (parts.length > 0 && parts[parts.length - 1] === '') parts.pop();
  return parts.map((l) => (l.endsWith('\r') ? l.slice(0, -1) : l));
}

// ==================================================================
// dir/delta 按会话订阅
// ==================================================================

/**
 * 订阅某目录会话的增量推送。sessionId 由客户端分配，因此可以在 dir/open
 * 之前订阅（避免应答与首批 delta 之间的竞态）。返回取消订阅函数。
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
