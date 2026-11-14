import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Transport } from '../protocol/transport';

/**
 * Tauri 引擎传输层：Rust 壳 spawn `ai-engine --stdio`，前端经 invoke/事件透传 stdio。
 *
 * **为什么走 Tauri command（自定义 spawn）而不是 tauri-plugin-shell sidecar？**
 * 1. 架构 ADR-003：Tauri 通道仅作字节管道，协议语义不绑定 Tauri。自定义 command 让
 *    stdio 透传、路径解析（dev 指向 ../../engine/target/debug/ai-engine，release 同目录）、
 *    进程重启/录制等策略全部集中在 Rust 壳，前端零改动可迁移到其他壳层（Electron 保底路径）；
 * 2. plugin-shell 的 sidecar 需要按目标三元组命名外部二进制并进 bundle，P0 阶段引擎
 *    尚未成形，先用自定义 spawn 降低打包耦合，后续若转正式 sidecar 仅改 Rust 侧；
 * 3. 插件还要引入 capability 权限配置；core invoke/event 无需额外能力声明。
 *
 * 事件契约（见 src-tauri/src/lib.rs）：
 * - invoke('engine_start', { path? }) -> { pid, path, engine }
 * - invoke('engine_send', { line })   -> 写一行到引擎 stdin
 * - invoke('engine_stop')             -> 杀进程
 * - event 'engine-stdout'             -> payload 为一条 NDJSON 行（Rust 侧按行读取）
 * - event 'engine-stderr'             -> 引擎诊断输出（仅日志）
 * - event 'engine-exit'               -> 引擎进程退出
 */

interface EngineStartInfo {
  pid: number;
  path: string;
  engine: string;
}

export class TauriEngineTransport implements Transport {
  private lineHandler?: (line: string) => void;
  private closeHandler?: (reason?: string) => void;
  private unlisteners: UnlistenFn[] = [];
  private stopped = false;
  private sendChain: Promise<void> = Promise.resolve();

  async start(): Promise<void> {
    // 先注册事件监听再启动进程，避免竞态丢失首条输出
    this.unlisteners.push(
      await listen<string>('engine-stdout', (ev) => {
        // 壳层 BufReader::lines() 已按整行推送并剥掉换行符：再过行缓冲会永远凑不出
        // 整行，导致所有应答被吞（initialize 超时）。此处 payload 即一条完整 NDJSON。
        if (ev.payload.length > 0) this.lineHandler?.(ev.payload);
      })
    );
    this.unlisteners.push(
      await listen<string>('engine-stderr', (ev) => {
        // 引擎诊断走 console（tracing 日志，docs/03 §6.10）
        console.debug('[ai-engine stderr]', ev.payload);
      })
    );
    this.unlisteners.push(
      await listen<unknown>('engine-exit', () => {
        if (!this.stopped) this.closeHandler?.('引擎进程已退出');
      })
    );

    const info = await invoke<EngineStartInfo>('engine_start');
    console.info(`[engine] 已启动 ${info.engine} (pid=${info.pid}, path=${info.path})`);
  }

  send(line: string): void {
    // 串行写入，保证 invoke 顺序与调用顺序一致
    this.sendChain = this.sendChain.then(async () => {
      try {
        await invoke('engine_send', { line });
      } catch (err) {
        this.closeHandler?.(`写入引擎失败: ${String(err)}`);
      }
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
    for (const un of this.unlisteners.splice(0)) {
      un();
    }
    try {
      await invoke('engine_stop');
    } catch {
      // 进程已死等场景忽略
    }
  }
}
