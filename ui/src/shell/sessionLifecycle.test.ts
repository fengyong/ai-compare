import { beforeEach, describe, expect, it, vi } from 'vitest';
import { activeWindowSelector, resetShellForTest, useShell } from './store';
import { cleanupTabEngines, tabSessionId } from './sliceWindowsTabs';
import { getEngineState, retryEngine } from '../ipc/engineProcess';
import { RpcClient } from '../protocol/client';
import type { Transport } from '../protocol/transport';

/**
 * 深度 review 修复的 UI 侧回归（FR-SES-001 / FR-WIN-001 / 引擎会话生命周期）。
 *
 * 覆盖：
 * 1. 关闭标签必须释放**全部**类型的引擎会话（此前只覆盖 text/folder/merge3，
 *    picture/table/hex/sync/merge 的会话在引擎侧只增不减 → 泄漏）；
 * 2. 引擎重连后 rebindSession 必须覆盖全部视图种类（此前只覆盖 folder/text，
 *    其余标签崩溃后持失效句柄 → 后续请求全 410/411）；
 * 3. 关闭窗口 / 关闭其他标签时，脏的命名会话必须先走关闭确认（此前静默丢弃，
 *    违反 FR-SES-001）；
 * 4. 横幅「重试」必须真正发起连接（此前只清状态不连接 → 永久停在"正在连接引擎…"）；
 * 5. 单个通知订阅者抛错不得中断其余订阅者（此前整条分发链被中断）。
 */

describe('shell/store 引擎会话生命周期回归', () => {
  beforeEach(() => {
    resetShellForTest();
    vi.restoreAllMocks();
  });

  it('tabSessionId：覆盖全部持有引擎会话的视图种类', () => {
    expect(tabSessionId({ id: 't1', kind: 'text', title: 'x', text: { sessionId: 's-text', left: { path: 'l', name: 'l' }, right: { path: 'r', name: 'r' } } })).toBe('s-text');
    expect(tabSessionId({ id: 't2', kind: 'folder', title: 'x', folder: { left: 'l', right: 'r', sessionId: 's-dir' } })).toBe('s-dir');
    expect(tabSessionId({ id: 't3', kind: 'sync', title: 'x', sync: { left: 'l', right: 'r', sessionId: 's-sync' } })).toBe('s-sync');
    expect(tabSessionId({ id: 't4', kind: 'picture', title: 'x', picture: { left: 'l', right: 'r', sessionId: 's-pic' } })).toBe('s-pic');
    expect(tabSessionId({ id: 't5', kind: 'table', title: 'x', table: { left: 'l', right: 'r', sessionId: 's-tab' } })).toBe('s-tab');
    expect(tabSessionId({ id: 't6', kind: 'hex', title: 'x', hex: { left: 'l', right: 'r', sessionId: 's-hex' } })).toBe('s-hex');
    expect(tabSessionId({ id: 't7', kind: 'merge', title: 'x', merge: { base: 'b', left: 'l', right: 'r', sessionId: 's-mrg' } })).toBe('s-mrg');
    expect(tabSessionId({ id: 't8', kind: 'merge3', title: 'x', merge3: { base: 'b', left: 'l', right: 'r', sessionId: 's-m3' } })).toBe('s-m3');
    expect(tabSessionId({ id: 't9', kind: 'home', title: 'x' })).toBeUndefined();
  });

  it('cleanupTabEngines：picture/table/hex/sync/merge 标签关闭时也要释放引擎会话', async () => {
    // 记录引擎侧 session/close 调用
    const closed: string[] = [];
    const engine = await import('../ipc/engineProcess');
    vi.spyOn(engine, 'closeSession').mockImplementation(async (p: { sessionId: string }) => {
      closed.push(p.sessionId);
      return null as never;
    });

    const cases = [
      { id: 'p1', kind: 'picture' as const, title: 'x', picture: { left: 'l', right: 'r', sessionId: 's-pic' } },
      { id: 't1', kind: 'table' as const, title: 'x', table: { left: 'l', right: 'r', sessionId: 's-tab' } },
      { id: 'h1', kind: 'hex' as const, title: 'x', hex: { left: 'l', right: 'r', sessionId: 's-hex' } },
      { id: 's1', kind: 'sync' as const, title: 'x', sync: { left: 'l', right: 'r', sessionId: 's-sync' } },
      { id: 'm1', kind: 'merge' as const, title: 'x', merge: { base: 'b', left: 'l', right: 'r', sessionId: 's-mrg' } },
    ];
    for (const tab of cases) {
      await cleanupTabEngines(tab as never);
    }
    expect(closed).toEqual(['s-pic', 's-tab', 's-hex', 's-sync', 's-mrg']);
  });

  it('rebindSession：引擎重连后全部视图种类的句柄都要重绑', () => {
    resetShellForTest();
    const tabs = [
      { id: 'p1', kind: 'picture' as const, title: 'x', picture: { left: 'l', right: 'r', sessionId: 'old-pic' } },
      { id: 't1', kind: 'table' as const, title: 'x', table: { left: 'l', right: 'r', sessionId: 'old-tab' } },
      { id: 'h1', kind: 'hex' as const, title: 'x', hex: { left: 'l', right: 'r', sessionId: 'old-hex' } },
      { id: 's1', kind: 'sync' as const, title: 'x', sync: { left: 'l', right: 'r', sessionId: 'old-sync' } },
      { id: 'm1', kind: 'merge3' as const, title: 'x', merge3: { base: 'b', left: 'l', right: 'r', sessionId: 'old-m3' } },
    ];
    useShell.setState((st) => ({
      windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, ...(tabs as never[])] })),
    }));

    useShell.getState().rebindSession('old-pic', 'new-pic');
    useShell.getState().rebindSession('old-tab', 'new-tab');
    useShell.getState().rebindSession('old-hex', 'new-hex');
    useShell.getState().rebindSession('old-sync', 'new-sync');
    useShell.getState().rebindSession('old-m3', 'new-m3');

    const all = useShell.getState().windows.flatMap((w) => w.tabs);
    const byId = (id: string) => all.find((t) => t.id === id)!;
    expect(byId('p1').picture?.sessionId).toBe('new-pic');
    expect(byId('t1').table?.sessionId).toBe('new-tab');
    expect(byId('h1').hex?.sessionId).toBe('new-hex');
    expect(byId('s1').sync?.sessionId).toBe('new-sync');
    expect(byId('m1').merge3?.sessionId).toBe('new-m3');
  });

  it('closeWindow：脏的命名会话必须先走关闭确认，不得静默丢弃', async () => {
    resetShellForTest();
    const dirty = {
      id: 'tab-dirty',
      kind: 'folder' as const,
      title: 'x',
      folder: { left: '/l', right: '/r', sessionId: 'dir-dirty' },
      sessionLink: { sessionId: 's-x', name: '命名', kind: 'folder-compare' as const, locked: false },
      dirty: true,
    };
    useShell.setState((st) => ({
      windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, dirty as never] })),
    }));

    const winId = useShell.getState().windows[0].id;
    await useShell.getState().closeWindow(winId);

    expect(useShell.getState().closeGuard?.tabId).toBe('tab-dirty');
    // 标签仍在（未静默丢弃）
    expect(useShell.getState().findTab('tab-dirty')).toBeDefined();
  });

  it('closeOthers：脏的命名会话必须先走关闭确认，不得静默丢弃', async () => {
    resetShellForTest();
    useShell.getState().openFolderTab('/a/L', '/a/R');
    const keepId = activeWindowSelector(useShell.getState())!.tabs[1].id;
    const dirty = {
      id: 'tab-dirty',
      kind: 'folder' as const,
      title: 'x',
      folder: { left: '/l', right: '/r', sessionId: 'dir-dirty' },
      sessionLink: { sessionId: 's-x', name: '命名', kind: 'folder-compare' as const, locked: false },
      dirty: true,
    };
    useShell.setState((st) => ({
      windows: st.windows.map((w) => ({ ...w, tabs: [...w.tabs, dirty as never] })),
    }));

    await useShell.getState().closeOthers(keepId);

    expect(useShell.getState().closeGuard?.tabId).toBe('tab-dirty');
    expect(useShell.getState().findTab('tab-dirty')).toBeDefined();
  });
});

describe('引擎连接：重试按钮必须真正发起连接', () => {
  it('retryEngine：调用后必须真正建立连接（不得只清状态）', async () => {
    retryEngine();
    // 内置演示引擎路径是同步连接，连接完成后状态为 mock。
    // 若 retryEngine 只清状态而不发起连接，状态会永远停在 "connecting"。
    await new Promise((r) => setTimeout(r, 100));
    expect(getEngineState().status).toBe('mock');
    expect(getEngineState().message).toContain('演示引擎');
  });
});

describe('协议客户端：通知订阅者异常隔离', () => {
  /** 脚本化传输：把预设行推给客户端 */
  class ScriptTransport implements Transport {
    private handler?: (line: string) => void;
    async start(): Promise<void> {
      /* noop */
    }
    send(line: string): void {
      // initialize 请求 → 回握手结果
      const msg = JSON.parse(line) as { id?: number };
      if (msg.id !== undefined) {
        this.handler?.(
          JSON.stringify({
            jsonrpc: '2.0',
            id: msg.id,
            result: {
              protocolVersion: '1.0',
              engine: { name: 'test', version: '0' },
              capabilities: {},
            },
          })
        );
      }
    }
    onMessage(handler: (line: string) => void): void {
      this.handler = handler;
    }
    onClose(): void {
      /* noop */
    }
    async stop(): Promise<void> {
      /* noop */
    }
    /** 主动推送一条通知 */
    push(method: string, params: unknown): void {
      this.handler?.(JSON.stringify({ jsonrpc: '2.0', method, params }));
    }
  }

  it('单个订阅者抛错不得中断其余订阅者', async () => {
    const transport = new ScriptTransport();
    const errors: Error[] = [];
    const client = new RpcClient(transport, {
      onProtocolError: (err) => errors.push(err),
    });
    await client.ready;

    const seen: string[] = [];
    client.on('dir/delta', () => {
      throw new Error('订阅者 A 抛错');
    });
    client.on('dir/delta', () => {
      seen.push('B');
    });

    transport.push('dir/delta', { sessionId: 'd1', rev: 1 });

    expect(seen).toEqual(['B']);
    expect(errors.length).toBe(1);
    expect(errors[0].message).toContain('订阅者 A 抛错');
    await client.close();
  });
});
