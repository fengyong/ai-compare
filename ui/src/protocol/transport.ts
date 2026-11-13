/**
 * 传输层抽象：客户端逻辑（RpcClient）与宿主环境解耦。
 *
 * 实现（ui/src/ipc/）：
 * - TauriEngineTransport：Tauri Rust 壳 spawn `ai-engine --stdio`，经 invoke/事件透传 stdio
 *   （架构 ADR-003：Tauri 通道仅作字节管道，协议语义不绑定 Tauri）；
 * - MockTransport：内置演示引擎（纯 TS 协议子集实现），浏览器 dev / 引擎缺失时降级；
 * - 测试中的脚本化 Mock 流（vitest）。
 */
export interface Transport {
  /** 建立连接（spawn 进程 / 打开事件监听）。失败应抛出带可读原因的错误。 */
  start(): Promise<void>;
  /** 发送一条 NDJSON 行（不含行尾换行）。实现方需保证顺序。 */
  send(line: string): void | Promise<void>;
  /** 注册完整行到达回调（须在 start 之前或 start 内生效） */
  onMessage(handler: (line: string) => void): void;
  /** 注册连接关闭回调（reason 为可读原因或 undefined） */
  onClose(handler: (reason?: string) => void): void;
  /** 关闭连接（杀进程/清理监听）。幂等。 */
  stop(): Promise<void>;
}
