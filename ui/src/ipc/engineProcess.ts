import { invoke } from '@tauri-apps/api/core';
import { RpcClient, RpcError } from '../protocol/client';
import type {
  DirDelta,
  DirExecuteParams,
  DirExecuteResult,
  DirExpandParams,
  DirExpandResult,
  DirFilterParams,
  DirFilterResult,
  DirMergeExecuteParams,
  DirMergeExecuteResult,
  DirMergePlanParams,
  DirMergePlanResult,
  DirMergeScanParams,
  DirMergeScanResult,
  DirOpenParams,
  DirOpenResult,
  DirOperateParams,
  DirOperateResult,
  EngineCapabilities,
  InitializeResult,
  MergeDecideParams,
  MergeDecideResult,
  MergeOpenParams,
  MergeOpenResult,
  MergeOutputParams,
  MergeOutputResult,
  MergeSectionsParams,
  MergeSectionsResult,
  ReportGenerateParams,
  ReportGenerateResult,
  SessionCloseParams,
  SessionCloseResult,
  TextInlineDiffParams,
  TextInlineDiffResult,
  TextEditParams,
  TextEditResult,
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
import { ReconnectPolicy, isMethodNotFound, type ResyncOutcome, type TrackedSession } from './reconnect';

/**
 * 引擎连接单例：连接选择（Tauri 真实引擎 / 演示引擎）、状态广播、按会话的 delta 订阅、
 * 以及 P0 方法集的类型化封装。视图层只与该模块交互，不直接触碰 Transport/RpcClient。
 *
 * 优雅降级策略（任务约束：引擎二进制缺失时给提示）：
 * - 浏览器 dev（无 Tauri）：直接使用内置演示引擎（mock），状态栏明确标识"演示模式"；
 * - Tauri 内 engine_start 失败（典型：未构建 target/debug/ai-engine）：进入 degraded，
 *   顶部横幅给出构建指引，可一键重试或切换演示引擎。
 *
 * P4 增强（rev 同步 / 断线重连，docs/03 §2.3 会话 rev 模型）：
 * - 崩溃检测：transport close（进程退出/stdio 断）触发 ReconnectPolicy；
 * - 自动重启一次（MAX_AUTO_RESTARTS=1）后重新 initialize；
 * - 会话恢复：先试 session/resync（P2 引擎若提供），-32601/410/超时 →
 *   降级路径 = UI 侧全量重放（分配新 sessionId 并广播 resync 事件，视图重开重建 delta）。
 *
 * 会话寻址：sessionId 由客户端分配（docs/03 §2.3.2 / ai-protocol DirOpenParams），
 * newSessionId() 统一生成。
 */

export type EngineStatus = 'connecting' | 'ready' | 'degraded' | 'mock';

export interface EngineStateSnapshot {
  status: EngineStatus;
  /** 引擎标识（名称+版本）或降级原因说明 */
  message: string;
  /** 横幅状态机（正常/重连中/恢复会话中/失败；mock 与初次连接也归一） */
  phase: 'ok' | 'reconnecting' | 'resyncing' | 'degraded' | 'failed' | 'mock';
}

interface DirDeltaSubscription {
  (delta: DirDelta): void;
}

let client: RpcClient | null = null;
let connectPromise: Promise<RpcClient> | null = null;
let forcedMock = false;
let engineCapabilities: EngineCapabilities | null = null;
let sessionSeq = 0;
const listeners = new Set<() => void>();
const deltaSubs = new Map<string, Set<DirDeltaSubscription>>();
const resyncListeners = new Set<(outcome: ResyncOutcome) => void>();
/** 已跟踪会话（崩溃后按序恢复；sessionId → 打开参数） */
const trackedSessions = new Map<string, TrackedSession>();
const reconnectPolicy = new ReconnectPolicy();
let snapshot: EngineStateSnapshot = { status: 'connecting', message: '正在连接引擎…', phase: 'ok' };

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

/** 订阅会话恢复事件（replay 模式下视图经 rebindSession 重开；resync 模式句柄不变） */
export function subscribeResync(fn: (outcome: ResyncOutcome) => void): () => void {
  resyncListeners.add(fn);
  return () => resyncListeners.delete(fn);
}

/** 登记需要崩溃恢复的会话（视图 open 成功后调用） */
export function trackSession(sessionId: string, kind: 'dir' | 'text', params: Record<string, unknown>): void {
  trackedSessions.set(sessionId, { kind, params });
}

/** 注销（标签关闭/会话删除后调用） */
export function untrackSession(sessionId: string): void {
  trackedSessions.delete(sessionId);
}

/** 当前引擎能力集（initialize 协商结果；未连接时为 null，调用方按不支持降级） */
export function getEngineCapabilities(): EngineCapabilities | null {
  return engineCapabilities;
}

/** 生成引擎侧会话句柄（客户端分配；前缀便于日志排查） */
export function newSessionId(kind: 'dir' | 'text' | 'merge' | 'merge3' | 'picture' | 'table' | 'hex'): string {
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
  setSnapshot({
    status: 'connecting',
    message: snapshot.phase === 'reconnecting' ? '引擎连接中断，正在自动重启…' : '正在连接引擎…',
    phase: snapshot.phase === 'reconnecting' ? 'reconnecting' : 'ok',
  });
  if (isTauriRuntime() && !forcedMock) {
    try {
      const transport = new TauriEngineTransport();
      const c = new RpcClient(transport, {
        onTransportClose: (reason) => handleTransportCrash(reason),
      });
      const info: InitializeResult = await c.ready;
      client = c;
      engineCapabilities = info.capabilities;
      wireNotifications(c);
      setSnapshot({
        status: 'ready',
        message: `${info.engine.name} ${info.engine.version}（协议 v${info.protocolVersion}）`,
        phase: 'ok',
      });
      // 重连成功：恢复已跟踪会话（先试 resync，失败走全量重放降级）
      if (reconnectPolicy.getPhase() === 'reconnecting') {
        reconnectPolicy.onConnected();
        await resyncTrackedSessions(c);
      }
      return c;
    } catch (err) {
      // 典型场景：target/debug/ai-engine 未构建。给出可见的降级横幅与指引。
      const message = err instanceof Error ? err.message : String(err);
      if (reconnectPolicy.getPhase() === 'reconnecting') {
        reconnectPolicy.onFailed();
        setSnapshot({ status: 'degraded', message: `自动重启失败：${message}`, phase: 'failed' });
      } else {
        setSnapshot({ status: 'degraded', message, phase: 'degraded' });
      }
      throw err;
    }
  }
  return connectMock();
}

/**
 * 引擎进程退出/stdio 断（崩溃检测入口）：
 * 自动重启一次 → initialize → 会话恢复；超出次数则失败横幅（可手动重试）。
 */
function handleTransportCrash(reason?: string): void {
  const old = client;
  client = null;
  connectPromise = null;
  engineCapabilities = null;
  // 释放旧客户端：其 Tauri 事件监听（stdout/stderr/exit）必须注销，
  // 否则每次崩溃重启净增 3 个监听器，且旧监听仍把 stdout 行投喂给已死客户端
  //（造成同一行被两个客户端各处理一次 → 幽灵 id 报错）。
  void old?.close();
  if (forcedMock || !isTauriRuntime()) {
    setSnapshot({ status: 'connecting', message: '演示引擎已关闭', phase: 'ok' });
    return;
  }
  reconnectPolicy.onDisconnect();
  if (reconnectPolicy.canAutoRestart()) {
    reconnectPolicy.beginRestart();
    setSnapshot({ status: 'connecting', message: '引擎连接中断，正在自动重启…', phase: 'reconnecting' });
    void getEngineClient().catch(() => {
      // 失败已在 doConnect 内落横幅（failed）
    });
  } else {
    reconnectPolicy.onFailed();
    setSnapshot({
      status: 'degraded',
      message: `引擎连接断开${reason ? `：${reason}` : ''}（自动重启已达上限）`,
      phase: 'failed',
    });
  }
}

/**
 * 会话恢复：先试引擎侧 session/resync（P2 若提供，rev 续传），
 * -32601（未实现）/410（会话不存在）/超时 → 降级：分配新句柄并广播，
 * 视图经 rebindSession 重开会话（UI 侧全量重放 delta）。
 */
async function resyncTrackedSessions(c: RpcClient): Promise<void> {
  const entries = [...trackedSessions.entries()];
  for (const [sessionId, tracked] of entries) {
    try {
      await c.request('session/resync', { sessionId, rev: 0 }, 4_000);
      const outcome: ResyncOutcome = { oldSessionId: sessionId, newSessionId: sessionId, kind: tracked.kind, mode: 'resync' };
      for (const fn of resyncListeners) fn(outcome);
    } catch (err) {
      if (isMethodNotFound(err)) {
        console.info('[resync] 引擎未提供 session/resync（P2），走 UI 全量重放降级');
      }
      const newId = newSessionId(tracked.kind);
      trackedSessions.delete(sessionId);
      // 新句柄必须重新登记：否则第二次引擎崩溃时该会话不再被恢复/重建。
      trackedSessions.set(newId, tracked);
      const outcome: ResyncOutcome = { oldSessionId: sessionId, newSessionId: newId, kind: tracked.kind, mode: 'replay' };
      for (const fn of resyncListeners) fn(outcome);
    }
  }
  reconnectPolicy.onResynced();
}

function connectMock(): RpcClient {
  const engine = new MockEngine();
  const c = new RpcClient(new MockTransport(engine), {
    onTransportClose: () => {
      client = null;
      setSnapshot({ status: 'connecting', message: '演示引擎已关闭', phase: 'ok' });
    },
  });
  client = c;
  void c.ready.then((info) => {
    engineCapabilities = info.capabilities;
  });
  wireNotifications(c);
  reconnectPolicy.settled('ok');
  setSnapshot({ status: 'mock', message: '演示引擎（内置模拟数据，非真实 ai-engine）', phase: 'mock' });
  return c;
}

function wireNotifications(c: RpcClient): void {
  c.on('dir/delta', (params) => {
    const delta = params as DirDelta;
    const subs = deltaSubs.get(delta.sessionId);
    if (subs) {
      for (const fn of subs) {
        try {
          fn(delta);
        } catch (err) {
          // 一个订阅者抛错不得中断其余订阅者（否则整批 delta 丢失）
          console.warn('[dir/delta] 订阅回调异常', err);
        }
      }
    }
  });
}

/** 顶部横幅"重试"：清空连接并重新协商 */
export function retryEngine(): void {
  forcedMock = false;
  reconnectPolicy.manualRetry();
  const old = client;
  client = null;
  connectPromise = null;
  void old?.close();
  setSnapshot({ status: 'connecting', message: '正在连接引擎…', phase: 'reconnecting' });
  // 必须真正发起连接：此前只清状态不连接，用户点「重试」会永远停在"正在连接引擎…"，
  // 直到下一次 RPC 调用才会间接触发连接。
  void getEngineClient().catch(() => {
    // 失败横幅已由 doConnect 内部落（phase=failed）
  });
}

/**
 * 当前是否运行在内置演示引擎（mock）上。
 *
 * 浏览器环境（无 Tauri）恒为演示引擎；桌面环境只有用户主动降级
 * （`switchToMockEngine`）或引擎启动失败后切换时才为演示引擎。
 * 演示引擎不触碰真实文件系统，故「示例路径」在两种模式下的取值不同。
 */
export function isMockEngineActive(): boolean {
  return forcedMock || !isTauriRuntime();
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

// ==================================================================
// P6：目录视图完备化方法集（expand/filter/operate；execute 为提案协议）
// ==================================================================

/** dir/expand：展开目录子树（惰性会话的消费入口；增量经 dir/delta 回推） */
export async function expandDir(params: DirExpandParams): Promise<DirExpandResult> {
  const c = await getEngineClient();
  return c.request<DirExpandResult>('dir/expand', params, 30_000);
}

/**
 * dir/filter：名称/其他过滤（引擎重扫）+ 显示过滤/抑制/平铺（仅显示）。
 * 真实引擎 P2 起 dirFilters=true；mock 同形实现。
 */
export async function filterDir(params: DirFilterParams): Promise<DirFilterResult> {
  const c = await getEngineClient();
  return c.request<DirFilterResult>('dir/filter', params, 60_000);
}

/** dir/operate：生成同步操作计划（不执行；SYNC-002 状态机数据层） */
export async function planOperations(params: DirOperateParams): Promise<DirOperateResult> {
  const c = await getEngineClient();
  return c.request<DirOperateResult>('dir/operate', params, 60_000);
}

/**
 * dir/execute（P6 提案协议，见 protocol/types.ts 第 3 节）：
 * 消费操作计划/右键操作执行文件操作（dryRun 试运行可对拍）。
 * mock 引擎已同形实现；真实引擎 RPC 入口待协议变更合入（-32601 时调用方降级提示）。
 */
export async function executeOperations(params: DirExecuteParams): Promise<DirExecuteResult> {
  const c = await getEngineClient();
  return c.request<DirExecuteResult>('dir/execute', params, 120_000);
}

// ==================================================================
// P14：图片比较方法集（picture/open；块清单全量返回）
// ==================================================================

export async function openHex(params: {
  sessionId: string;
  left: string;
  right: string;
  options?: { chunkSize?: number; alignMode?: string } | null;
}): Promise<{
  sessionId: string;
  leftSize: number;
  rightSize: number;
  blockCount: number;
  summary: { sameBytes: number; changedBytes: number; insertedBytes: number; deletedBytes: number };
}> {
  const c = await getEngineClient();
  return c.request('hex/open', params, 60_000);
}

export async function hexBytes(params: {
  sessionId: string;
  side: 'left' | 'right';
  offset: number;
  length: number;
}): Promise<{
  lines: Array<{ addr: number; hex: string; ascii: string; status: string }>;
  done: boolean;
}> {
  const c = await getEngineClient();
  return c.request('hex/bytes', params, 60_000);
}

export async function openTable(params: {
  sessionId: string;
  left: string;
  right: string;
  options?: {
    delimiter?: string;
    hasHeader?: boolean;
    keyColumns?: number[];
    columnAlign?: string;
    rowAlign?: string;
    skewTolerance?: number;
    caseUnimportant?: boolean;
  } | null;
}): Promise<{
  sessionId: string;
  rowCount: number;
  leftRowCount: number;
  rightRowCount: number;
  columns: Array<{ name: string; ty: string; left: number | null; right: number | null }>;
  summary: { sameRows: number; changedRows: number; addedRows: number; removedRows: number; unimportantRows: number };
}> {
  const c = await getEngineClient();
  return c.request('table/open', params, 60_000);
}

export async function tableSections(params: {
  sessionId: string;
  offset: number;
  count: number;
}): Promise<{
  sessionId: string;
  offset: number;
  rowCount: number;
  hasMore: boolean;
  rows: Array<{
    kind: 'same' | 'changed' | 'added' | 'removed';
    left: number | null;
    right: number | null;
    important: boolean;
    cells: Array<{ left: string | null; right: string | null; status: string }>;
  }>;
}> {
  const c = await getEngineClient();
  return c.request('table/sections', params, 60_000);
}

export async function openPicture(params: {
  sessionId: string;
  left: string;
  right: string;
  options?: { tolerance?: number; block?: number } | null;
}): Promise<{
  sessionId: string;
  width: number;
  height: number;
  leftWidth: number;
  leftHeight: number;
  rightWidth: number;
  rightHeight: number;
  diffPixels: number;
  totalPixels: number;
  similarityPct: number;
  blocks: Array<{ x: number; y: number; w: number; h: number; diffRatio: number }>;
  truncated: boolean;
}> {
  const c = await getEngineClient();
  return c.request('picture/open', params, 60_000);
}

// ==================================================================
// P7：三向文本合并方法集（text/merge3/open|sections|decide|output）
// ==================================================================
// 接线模式同 P14 三视图：视图只调本模块封装，不直接触碰 RpcClient。
// open 一次性求出合并模型；块清单经 sections 分批；decide 提交采用决策
// （引擎侧幂等：决策内容不变时 rev 不递增）；output 合成最终文本。

/** text/merge3/open：base/left/right 三路文本 → 合并会话（应答统计） */
export async function merge3Open(params: MergeOpenParams): Promise<MergeOpenResult> {
  const c = await getEngineClient();
  return c.request<MergeOpenResult>('text/merge3/open', params, 60_000);
}

/** text/merge3/sections：块清单分批窗口（三方行内容 + 决策状态） */
export async function merge3Sections(params: MergeSectionsParams): Promise<MergeSectionsResult> {
  const c = await getEngineClient();
  return c.request<MergeSectionsResult>('text/merge3/sections', params, 60_000);
}

/** text/merge3/decide：块决策提交（FR-TMRG-004/005 采用与手动编辑） */
export async function merge3Decide(params: MergeDecideParams): Promise<MergeDecideResult> {
  const c = await getEngineClient();
  return c.request<MergeDecideResult>('text/merge3/decide', params, 60_000);
}

/** text/merge3/output：合成输出（plain / marker 冲突标记） */
export async function merge3Output(params: MergeOutputParams): Promise<MergeOutputResult> {
  const c = await getEngineClient();
  return c.request<MergeOutputResult>('text/merge3/output', params, 60_000);
}

/**
 * P11：报告生成（report/generate；FR-RPT-001/002）。
 * ts-rs 把 u64 的 offset/limit 生成为 bigint，但 NDJSON 传输必须是 number
 * （JSON.stringify 不支持 bigint），故此处把这两个字段收窄为 number，
 * 与 P14 hex/bytes 的 offset 处理同口径。
 */
export type ReportParams = Omit<ReportGenerateParams, 'offset' | 'limit'> & {
  offset?: number;
  limit?: number;
};

/** report/generate：会话快照 + 布局参数 → 报告内容（可 offset/limit 分块） */
export async function generateReport(params: ReportParams): Promise<ReportGenerateResult> {
  const c = await getEngineClient();
  return c.request<ReportGenerateResult>('report/generate', params, 120_000);
}

/**
 * 会话已存在（411）时幂等重建：先 `session/close` 再重新 open。
 *
 * 两个真实场景：① React StrictMode 双挂载使 effect 里的 open 执行两次；
 * ② 图片容差滑杆变化触发重比较（复用同一 sessionId）。此前两者都会
 * 被引擎 411 拒绝，导致视图显示错误。close 是幂等的（会话不存在也返回 null）。
 */
function isSessionExists(err: unknown): boolean {
  return typeof err === 'object' && err !== null && (err as { code?: number }).code === 411;
}

type PictureOpenArgs = Parameters<typeof openPicture>[0];
type PictureOpenReply = Awaited<ReturnType<typeof openPicture>>;

export async function openPictureIdempotent(params: PictureOpenArgs): Promise<PictureOpenReply> {
  try {
    return await openPicture(params);
  } catch (err) {
    if (!isSessionExists(err)) throw err;
    await closeSession({ sessionId: params.sessionId });
    return openPicture(params);
  }
}

export async function merge3OpenIdempotent(params: MergeOpenParams): Promise<MergeOpenResult> {
  try {
    return await merge3Open(params);
  } catch (err) {
    if (!isSessionExists(err)) throw err;
    await closeSession({ sessionId: params.sessionId });
    return merge3Open(params);
  }
}

// ==================================================================
// P8：文件夹三路合并方法集（提案协议，见 protocol/types.ts 第 4 节）
// ==================================================================

/** dir/mergeScan：base/left/right 三目录扫描对齐 + diff3 分类（mock 同形实现） */
export async function mergeScan(params: DirMergeScanParams): Promise<DirMergeScanResult> {
  const c = await getEngineClient();
  return c.request<DirMergeScanResult>('dir/mergeScan', params, 60_000);
}

/** dir/mergePlan：合并计划生成（决策继承展开；不执行） */
export async function mergePlan(params: DirMergePlanParams): Promise<DirMergePlanResult> {
  const c = await getEngineClient();
  return c.request<DirMergePlanResult>('dir/mergePlan', params, 60_000);
}

/** dir/mergeExecute：合并计划执行（部分成功语义 + 日志行） */
export async function mergeExecute(params: DirMergeExecuteParams): Promise<DirMergeExecuteResult> {
  const c = await getEngineClient();
  return c.request<DirMergeExecuteResult>('dir/mergeExecute', params, 120_000);
}

export async function openText(params: TextOpenParams): Promise<TextOpenResult> {
  const c = await getEngineClient();
  return c.request<TextOpenResult>('text/open', params, 60_000);
}

export async function getTextSections(params: TextSectionsParams): Promise<TextSectionsResult> {
  const c = await getEngineClient();
  return c.request<TextSectionsResult>('text/sections', params, 120_000);
}

/**
 * text/edit：提交编辑增量，取回引擎权威 sections（R3-18）。
 *
 * 能力位缺失（旧引擎）或入口不存在（-32601）→ null，调用方回落客户端重 diff；
 * 其余错误（区间越界 / 会话不存在 / 断连）**抛出**，由调用方整侧重放收敛，
 * 不得静默吞掉（吞掉会让引擎态与 UI 态长期分叉）。
 */
export async function editText(params: TextEditParams): Promise<TextEditResult | null> {
  if (!getEngineCapabilities()?.textEdit) return null;
  const c = await getEngineClient();
  try {
    return await c.request<TextEditResult>('text/edit', params, 60_000);
  } catch (err) {
    if (isMethodNotFound(err)) return null;
    throw err;
  }
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

// ==================================================================
// 行内差异：text/inlineDiff（能力位判断 + 客户端词级兜底，P5；P3 协议面对齐）
// ==================================================================

/**
 * 按 section 请求行内差异（P3 生成协议 TextInlineDiffParams/Result：
 * {sessionId, sectionId} → 绝对行号 + InlineSpanDto 成对区间）。
 *
 * 能力位策略（任务书交付 7）：
 * - initialize.capabilities.inlineDiff=true → 走 RPC（mock 已同形实现；
 *   P3 真实引擎已置位，自动生效）；
 * - 能力位为 false 或 RPC 报错（-32601/410 等）→ 返回 null，调用方回落
 *   客户端词级 LCS（lib/inlineDiffCore，同形兜底）。
 */
export async function requestInlineDiff(params: TextInlineDiffParams): Promise<TextInlineDiffResult | null> {
  const caps = getEngineCapabilities();
  if (!caps?.inlineDiff) return null;
  const c = await getEngineClient();
  try {
    return await c.request<TextInlineDiffResult>('text/inlineDiff', params, 30_000);
  } catch {
    // 引擎声明了能力但入口未就绪/请求失败：降级客户端计算
    return null;
  }
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
