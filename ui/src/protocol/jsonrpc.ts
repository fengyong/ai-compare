/**
 * JSON-RPC 2.0 消息模型与 NDJSON 帧编解码（docs/03 §2.3.2）。
 *
 * 传输帧：NDJSON —— 每行一条消息，UTF-8，`\n` 分隔。
 * JSON.stringify 会转义字符串内的换行，因此单条消息序列化后天然单行、无歧义。
 */

export type RpcId = number | string;

export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: RpcId;
  method: string;
  params?: unknown;
}

export interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

export interface JsonRpcErrorObject {
  code: number;
  message: string;
  data?: unknown;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: RpcId;
  result?: unknown;
  error?: JsonRpcErrorObject;
}

/** 序列化为一条 NDJSON 行（不含行尾换行符） */
export function encodeMessage(msg: JsonRpcRequest | JsonRpcNotification | JsonRpcResponse): string {
  return JSON.stringify(msg);
}

/**
 * 行缓冲：把可能被任意切割的字符块重组为完整行。
 *
 * 场景：Tauri 事件虽按行推送，但任何以"字节流/字符块"形态到达的通道
 * （例如未来的 WebSocket 壳、录制回放文件、进程直读）都可能把一条消息
 * 拆成多次到达，或把多条粘在一起。所有 Transport 统一经此缓冲，客户端
 * 逻辑只面对完整行。
 */
export class LineBuffer {
  private buf = '';

  /** 喂入一个任意切割的字符串块，返回其中全部完整行（跳过空行，剔除 \r） */
  push(chunk: string): string[] {
    this.buf += chunk;
    const lines: string[] = [];
    let idx = this.buf.indexOf('\n');
    while (idx >= 0) {
      let line = this.buf.slice(0, idx);
      this.buf = this.buf.slice(idx + 1);
      if (line.endsWith('\r')) {
        line = line.slice(0, -1);
      }
      if (line.length > 0) {
        lines.push(line);
      }
      idx = this.buf.indexOf('\n');
    }
    return lines;
  }

  /** 是否还有未凑成整行的残余数据 */
  hasPending(): boolean {
    return this.buf.length > 0;
  }

  /** 丢弃残余（连接关闭时调用；正常协议下不应有残余） */
  reset(): void {
    this.buf = '';
  }
}
