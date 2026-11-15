import { describe, expect, it, vi } from 'vitest';
import { RpcClient, RpcError, LOCAL_ERROR } from './client';
import type { Transport } from './transport';
import { LineBuffer } from './jsonrpc';

/**
 * 协议客户端单测（任务要求：用 mock stream 覆盖）。
 * MockStream 以"完整行注入 + 关闭事件"模拟引擎侧 stdio；
 * LineBuffer 的粘包/半包重组在同一文件内覆盖。
 */

const INIT_RESULT = {
  protocolVersion: '1.0',
  engineInfo: { name: 'test-engine', version: '0.0.0' },
  capabilities: { viewKinds: [], features: [] },
};

class MockStream implements Transport {
  private lineHandler?: (line: string) => void;
  private closeHandler?: (reason?: string) => void;
  readonly sent: string[] = [];
  private stopped = false;

  async start(): Promise<void> {}
  async stop(): Promise<void> {
    this.stopped = true;
  }
  send(line: string): void {
    if (this.stopped) throw new Error('stream closed');
    this.sent.push(line);
  }
  onMessage(handler: (line: string) => void): void {
    this.lineHandler = handler;
  }
  onClose(handler: (reason?: string) => void): void {
    this.closeHandler = handler;
  }

  // ---- 测试辅助 ----
  push(line: string): void {
    this.lineHandler?.(line);
  }
  reply(id: number | string, result: unknown): void {
    this.push(JSON.stringify({ jsonrpc: '2.0', id, result }));
  }
  replyError(id: number | string, code: number, message: string): void {
    this.push(JSON.stringify({ jsonrpc: '2.0', id, error: { code, message } }));
  }
  notify(method: string, params?: unknown): void {
    this.push(JSON.stringify({ jsonrpc: '2.0', method, params }));
  }
  lastSent(): Record<string, unknown> | undefined {
    const raw = this.sent[this.sent.length - 1];
    return raw ? (JSON.parse(raw) as Record<string, unknown>) : undefined;
  }
  /** 自动应答第一个未应答的请求（initialize 握手用） */
  answerInitialize(): void {
    const first = JSON.parse(this.sent[0]) as { id: number; method: string };
    if (first.method === 'initialize') this.reply(first.id, INIT_RESULT);
  }
  close(reason?: string): void {
    this.closeHandler?.(reason);
  }
}

/** 让 bootstrap 的微任务链跑完（写入 initialize 请求） */
const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

async function makeClient(opts?: ConstructorParameters<typeof RpcClient>[1]) {
  const stream = new MockStream();
  const client = new RpcClient(stream, opts);
  await flush();
  stream.answerInitialize();
  await client.ready;
  return { stream, client };
}

describe('RpcClient', () => {
  it('initialize 握手：首个发送为 initialize，ready 携带能力集', async () => {
    const { stream, client } = await makeClient();
    const first = JSON.parse(stream.sent[0]) as Record<string, unknown>;
    expect(first.method).toBe('initialize');
    expect(first.jsonrpc).toBe('2.0');
    expect(typeof first.id).toBe('number');
    await expect(client.ready).resolves.toMatchObject({
      protocolVersion: '1.0',
      engineInfo: { name: 'test-engine' },
    });
  });

  it('request/response 往返按 id 关联', async () => {
    const { stream, client } = await makeClient();
    const p = client.request<{ rows: number }>('dir/rows', {
      sessionId: 's1',
      offset: 0,
      count: 10,
    });
    await flush(); // 写队列经微任务入队
    const sent = JSON.parse(stream.sent[stream.sent.length - 1]) as { id: number };
    stream.reply(sent.id, { rows: 42 });
    await expect(p).resolves.toEqual({ rows: 42 });
  });

  it('错误应答拒绝并携带错误码与消息', async () => {
    const { stream, client } = await makeClient();
    const p = client.request('dir/open', {});
    await flush();
    const sent = JSON.parse(stream.sent[stream.sent.length - 1]) as { id: number };
    stream.replyError(sent.id, 410, 'session not found');
    await expect(p).rejects.toMatchObject({ code: 410, message: 'session not found' });
    await expect(p).rejects.toBeInstanceOf(RpcError);
  });

  it('通知分发到订阅者，且可取消订阅', async () => {
    const { stream, client } = await makeClient();
    const spy = vi.fn();
    const off = client.on('dir/delta', spy);
    stream.notify('dir/delta', { sessionId: 's1', rev: 1, upserts: [], removals: [] });
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toMatchObject({ sessionId: 's1', rev: 1 });
    off();
    stream.notify('dir/delta', { sessionId: 's1', rev: 2, upserts: [], removals: [] });
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('initialize 失败（错误应答）→ ready 拒绝', async () => {
    const stream = new MockStream();
    const client = new RpcClient(stream);
    await flush();
    const first = JSON.parse(stream.sent[0]) as { id: number };
    stream.replyError(first.id, -32602, '版本不兼容');
    await expect(client.ready).rejects.toMatchObject({ code: -32602 });
  });

  it('非法 JSON 行不致命（回调上报，后续应答仍处理）', async () => {
    const errors: string[] = [];
    const { stream, client } = await makeClient({
      onProtocolError: (err) => errors.push(err.message),
    });
    stream.push('this is not json');
    const p = client.request('ping');
    await flush();
    const sent = JSON.parse(stream.sent[stream.sent.length - 1]) as { id: number };
    stream.reply(sent.id, 'pong');
    await expect(p).resolves.toBe('pong');
    expect(errors.length).toBeGreaterThanOrEqual(1);
  });

  it('超时触发 $/cancel 并以 TIMEOUT 码拒绝', async () => {
    vi.useFakeTimers();
    try {
      const stream = new MockStream();
      const client = new RpcClient(stream);
      await vi.advanceTimersByTimeAsync(0);
      stream.answerInitialize();
      await vi.advanceTimersByTimeAsync(0);
      await client.ready;
      const p = client.request('slow/method', undefined, 50);
      await vi.advanceTimersByTimeAsync(1); // 写队列入队（不触发 50ms 超时）
      const sentId = (JSON.parse(stream.sent[stream.sent.length - 1]) as { id: number }).id;
      const expectation = expect(p).rejects.toMatchObject({ code: LOCAL_ERROR.TIMEOUT });
      vi.advanceTimersByTime(60);
      await expectation;
      const cancel = stream.sent.find((l) => l.includes('$/cancel'));
      expect(cancel).toBeDefined();
      expect(JSON.parse(cancel!)).toMatchObject({
        method: '$/cancel',
        params: { id: sentId },
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it('close 后在途请求全部拒绝，后续请求立即失败', async () => {
    const { client } = await makeClient();
    const p1 = client.request('a', undefined, 60_000);
    const p2 = client.request('b', undefined, 60_000);
    void client.close();
    await expect(p1).rejects.toMatchObject({ code: LOCAL_ERROR.CLOSED });
    await expect(p2).rejects.toMatchObject({ code: LOCAL_ERROR.CLOSED });
    await expect(client.request('c')).rejects.toMatchObject({ code: LOCAL_ERROR.CLOSED });
    expect(client.isClosed()).toBe(true);
  });

  it('传输关闭（引擎退出）后 pending 全部拒绝并回调 onTransportClose', async () => {
    const onTransportClose = vi.fn();
    const { stream, client } = await makeClient({ onTransportClose });
    const p = client.request('x', undefined, 60_000);
    stream.close('引擎进程已退出');
    await expect(p).rejects.toMatchObject({ code: LOCAL_ERROR.CLOSED });
    expect(onTransportClose).toHaveBeenCalledWith('引擎进程已退出');
  });

  it('server→client 请求回 -32601（P0 未实现任何服务端请求方法）', async () => {
    const { stream } = await makeClient();
    stream.push(JSON.stringify({ jsonrpc: '2.0', id: 900, method: 'window/confirm', params: {} }));
    await flush();
    const reply = stream.lastSent();
    expect(reply).toMatchObject({
      jsonrpc: '2.0',
      id: 900,
      error: { code: -32601 },
    });
  });
});

describe('LineBuffer（NDJSON 重组）', () => {
  const lb = () => new LineBuffer();

  it('完整行原样通过', () => {
    const b = lb();
    expect(b.push('{"a":1}\n{"b":2}\n')).toEqual(['{"a":1}', '{"b":2}']);
    expect(b.hasPending()).toBe(false);
  });

  it('半包：残留在缓冲直到下一块凑成整行', () => {
    const b = lb();
    expect(b.push('{"a"')).toEqual([]);
    expect(b.hasPending()).toBe(true);
    expect(b.push(':1}\n')).toEqual(['{"a":1}']);
  });

  it('粘包：一次 push 出多条', () => {
    const b = lb();
    expect(b.push('x\nyz\nw')).toEqual(['x', 'yz']);
    expect(b.hasPending()).toBe(true);
    expect(b.push('\n')).toEqual(['w']);
  });

  it('剔除 \\r\\n 与空行', () => {
    const b = lb();
    expect(b.push('a\r\n\r\nb\n')).toEqual(['a', 'b']);
  });

  it('reset 丢弃残余', () => {
    const b = lb();
    b.push('partial');
    b.reset();
    expect(b.hasPending()).toBe(false);
    expect(b.push('x\n')).toEqual(['x']);
  });
});
