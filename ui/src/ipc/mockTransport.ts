import type { Transport } from '../protocol/transport';
import type { MockEngine } from './mockEngine';

/**
 * 演示传输层：与内置 MockEngine 组成进程内全双工"伪 stdio"。
 *
 * receive → 引擎同步处理后经 sink 产出应答/通知 → queueMicrotask 送达客户端，
 * 保持与真实 stdio 相同的异步时序假设（请求先入队、应答后到达）。
 */
export class MockTransport implements Transport {
  private lineHandler?: (line: string) => void;
  private closeHandler?: (reason?: string) => void;
  private stopped = false;

  constructor(private readonly engine: MockEngine) {
    this.engine.attach((line) => this.deliver(line));
  }

  async start(): Promise<void> {
    // 进程内无真实建立成本
  }

  send(line: string): void {
    if (this.stopped) return;
    // 异步投递给引擎，模拟跨进程边界（也避免请求-应答同栈重入）
    queueMicrotask(() => {
      if (!this.stopped) this.engine.receive(line);
    });
  }

  onMessage(handler: (line: string) => void): void {
    this.lineHandler = handler;
  }

  onClose(handler: (reason?: string) => void): void {
    this.closeHandler = handler;
  }

  async stop(): Promise<void> {
    if (this.stopped) return;
    this.stopped = true;
    this.engine.dispose();
    this.closeHandler?.('演示引擎已关闭');
  }

  private deliver(line: string): void {
    queueMicrotask(() => {
      if (!this.stopped) this.lineHandler?.(line);
    });
  }
}
