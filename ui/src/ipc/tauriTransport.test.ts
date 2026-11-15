import { describe, it, expect, vi, beforeEach } from 'vitest';

const h = vi.hoisted(() => ({
  listeners: new Map<string, (ev: { payload: unknown }) => void>(),
  calls: [] as Array<{ cmd: string; args?: unknown; listenersAtCall: number }>,
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: async (cmd: string, args?: unknown) => {
    h.calls.push({ cmd, args, listenersAtCall: h.listeners.size });
    if (cmd === 'engine_start') {
      return { pid: 4321, path: '/bundle/ai-engine', engine: 'ai-engine' };
    }
    return null;
  },
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: async (name: string, cb: (ev: { payload: unknown }) => void) => {
    h.listeners.set(name, cb);
    return () => {
      h.listeners.delete(name);
    };
  },
}));

import { TauriEngineTransport } from './tauriTransport';

function emit(name: string, payload: unknown): void {
  h.listeners.get(name)?.({ payload });
}

describe('ipc/tauriTransport 壳层行契约', () => {
  beforeEach(() => {
    h.listeners.clear();
    h.calls.length = 0;
  });

  it('engine-stdout 载荷无换行符也整条投递（壳层 BufReader::lines 已剥换行）', async () => {
    const t = new TauriEngineTransport();
    const got: string[] = [];
    t.onMessage((line) => got.push(line));
    await t.start();

    const answer = '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0"}}';
    emit('engine-stdout', answer);
    expect(got).toEqual([answer]);

    const delta = '{"jsonrpc":"2.0","method":"dir/delta","params":{"rev":2}}';
    emit('engine-stdout', delta);
    expect(got).toEqual([answer, delta]);

    await t.stop();
  });

  it('空载荷忽略；三个监听先于 engine_start 挂好（不丢首条输出）', async () => {
    const t = new TauriEngineTransport();
    const got: string[] = [];
    t.onMessage((line) => got.push(line));
    await t.start();

    emit('engine-stdout', '');
    expect(got).toEqual([]);

    const start = h.calls.find((c) => c.cmd === 'engine_start');
    expect(start?.listenersAtCall).toBe(3);

    await t.stop();
    expect(h.listeners.size).toBe(0);
  });

  it('send 按调用顺序串行写 engine_send', async () => {
    const t = new TauriEngineTransport();
    await t.start();

    t.send('{"id":1}');
    t.send('{"id":2}');
    await vi.waitFor(() => {
      expect(h.calls.filter((c) => c.cmd === 'engine_send')).toHaveLength(2);
    });

    expect(h.calls.filter((c) => c.cmd === 'engine_send').map((c) => c.args)).toEqual([
      { line: '{"id":1}' },
      { line: '{"id":2}' },
    ]);

    await t.stop();
  });
});
