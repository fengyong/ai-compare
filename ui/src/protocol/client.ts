import type { Transport } from './transport';
import type { ClientInfo, InitializeResult } from './types';

/**
 * JSON-RPC 2.0 客户端（NDJSON over stdio 之上的协议语义层，docs/03 §2.3）。
 *
 * 职责：
 * - initialize 握手（ready Promise，握手成功前拒绝业务请求）
 * - 请求/应答关联（自增 id）、超时 + `$/cancel`（LSP 风格）
 * - 引擎 → UI 通知分发（dir/delta、progress 等）
 * - 写入序列化（所有发送按序排队），非法行不中断会话
 */

/** 协议层错误码（docs/03 附录 B：领域错误码 + RPC 内部细分） */
export class RpcError extends Error {
  readonly code: number;
  readonly data?: unknown;

  constructor(code: number, message: string, data?: unknown) {
    super(message);
    this.name = 'RpcError';
    this.code = code;
    this.data = data;
  }
}

/** 本客户端合成的非协议错误码（负值避开引擎领域码段） */
export const LOCAL_ERROR = {
  /** 请求超时（已发 $/cancel） */
  TIMEOUT: -32001,
  /** 连接已关闭 */
  CLOSED: -32002,
} as const;

export interface RpcClientOptions {
  clientInfo?: ClientInfo;
  protocolVersion?: string;
  /** 默认请求超时；超时后自动发送 $/cancel 并拒绝 */
  defaultTimeoutMs?: number;
  /** 非法帧/未知应答等协议级异常（不致命，供日志/遥测） */
  onProtocolError?: (err: Error, rawLine: string) => void;
  /** 传输关闭（引擎退出等）；此后客户端不可再用 */
  onTransportClose?: (reason?: string) => void;
}

interface PendingEntry {
  method: string;
  resolve: (value: unknown) => void;
  reject: (err: unknown) => void;
  timer: ReturnType<typeof setTimeout>;
}

const CANCEL_METHOD = '$/cancel';

export class RpcClient {
  private nextId = 1;
  private readonly pending = new Map<number, PendingEntry>();
  private readonly handlers = new Map<string, Set<(params: unknown) => void>>();
  private writeQueue: Promise<void> = Promise.resolve();
  private closed = false;

  /** initialize 握手 Promise：resolve 即连接可用（能力协商完成） */
  readonly ready: Promise<InitializeResult>;

  constructor(
    private readonly transport: Transport,
    private readonly opts: RpcClientOptions = {},
  ) {
    this.ready = this.bootstrap();
  }

  private async bootstrap(): Promise<InitializeResult> {
    await this.transport.start();
    this.transport.onMessage((line) => this.handleLine(line));
    this.transport.onClose((reason) => this.handleTransportClose(reason));
    return this.request<InitializeResult>(
      'initialize',
      {
        clientInfo: this.opts.clientInfo ?? { name: 'ai-compare-ui', version: '0.1.0' },
        protocolVersion: this.opts.protocolVersion ?? '1.0',
      },
      15_000,
    );
  }

  /** 发起请求；params 由调用方按 protocol/types.ts 构造 */
  request<T = unknown>(
    method: string,
    params?: unknown,
    timeoutMs = this.opts.defaultTimeoutMs ?? 30_000,
  ): Promise<T> {
    if (this.closed) {
      return Promise.reject(new RpcError(LOCAL_ERROR.CLOSED, '连接已关闭，无法发送请求'));
    }
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        // LSP 风格取消：引擎侧 CancellationToken 贯穿（docs/03 §2.3.2）
        this.notify(CANCEL_METHOD, { id });
        reject(
          new RpcError(LOCAL_ERROR.TIMEOUT, `请求超时（${timeoutMs}ms）: ${method}`, { method }),
        );
      }, timeoutMs);
      this.pending.set(id, { method, resolve: resolve as (v: unknown) => void, reject, timer });
      this.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
    });
  }

  /** 发送通知（无 id，引擎不应答），如 $/cancel */
  notify(method: string, params?: unknown): void {
    if (this.closed) return;
    this.write(JSON.stringify({ jsonrpc: '2.0', method, params }));
  }

  /** 订阅引擎通知；返回取消订阅函数 */
  on(method: string, handler: (params: unknown) => void): () => void {
    let set = this.handlers.get(method);
    if (!set) {
      set = new Set();
      this.handlers.set(method, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  }

  /** 关闭连接：拒绝所有在途请求并停掉传输层。幂等。 */
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.rejectAllPending(new RpcError(LOCAL_ERROR.CLOSED, '连接已关闭'));
    this.handlers.clear();
    try {
      await this.transport.stop();
    } catch {
      // 停止失败不改变"已关闭"语义
    }
  }

  isClosed(): boolean {
    return this.closed;
  }

  // ------------------------------------------------------------------

  private write(line: string): void {
    // 串行化：保证引擎收到的请求顺序与调用顺序一致（invoke 跨 IPC 边界时不依赖宿主保序）
    this.writeQueue = this.writeQueue
      .then(() => this.transport.send(line))
      .catch((err) => {
        this.opts.onProtocolError?.(
          err instanceof Error ? err : new Error(String(err)),
          `[write] ${line.slice(0, 120)}`,
        );
      });
  }

  private handleLine(line: string): void {
    const raw = line.trim();
    if (!raw) return;
    let msg: unknown;
    try {
      msg = JSON.parse(raw);
    } catch (err) {
      this.opts.onProtocolError?.(err instanceof Error ? err : new Error(String(err)), raw);
      return;
    }
    if (typeof msg !== 'object' || msg === null) {
      this.opts.onProtocolError?.(new Error('非对象 JSON-RPC 消息'), raw);
      return;
    }
    const m = msg as {
      id?: unknown;
      method?: unknown;
      params?: unknown;
      result?: unknown;
      error?: unknown;
    };

    if (typeof m.method === 'string') {
      if (m.id !== undefined && m.result === undefined && m.error === undefined) {
        // server→client 请求：P0 未支持任何此类方法，回 -32601 避免引擎悬挂等待
        this.write(
          JSON.stringify({
            jsonrpc: '2.0',
            id: m.id,
            error: { code: -32601, message: `客户端未实现该方法: ${m.method}` },
          }),
        );
        return;
      }
      const set = this.handlers.get(m.method);
      if (set) {
        for (const h of set) {
          try {
            h(m.params);
          } catch (err) {
            // 单个订阅者抛错不得中断其余订阅者，也不得把异常抛回传输层
            //（否则会冒泡成未捕获异常，并让后续通知全部丢失）
            this.opts.onProtocolError?.(
              err instanceof Error ? err : new Error(String(err)),
              `[notify] ${m.method}`,
            );
          }
        }
      }
      return;
    }

    // 应答
    const id = m.id;
    if (typeof id !== 'number') {
      this.opts.onProtocolError?.(new Error(`应答缺少合法 id: ${String(id)}`), raw);
      return;
    }
    const entry = this.pending.get(id);
    if (!entry) {
      this.opts.onProtocolError?.(new Error(`收到未知 id=${id} 的应答（超时或已关闭）`), raw);
      return;
    }
    clearTimeout(entry.timer);
    this.pending.delete(id);
    if (m.error !== undefined && m.error !== null) {
      const e = m.error as { code: number; message: string; data?: unknown };
      entry.reject(new RpcError(e.code, e.message, e.data));
    } else {
      entry.resolve(m.result);
    }
  }

  private rejectAllPending(err: unknown): void {
    for (const entry of this.pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(err);
    }
    this.pending.clear();
  }

  private handleTransportClose(reason?: string): void {
    if (this.closed) return;
    this.closed = true;
    this.rejectAllPending(
      new RpcError(LOCAL_ERROR.CLOSED, `连接被关闭${reason ? `: ${reason}` : ''}`),
    );
    this.handlers.clear();
    this.opts.onTransportClose?.(reason);
  }
}
