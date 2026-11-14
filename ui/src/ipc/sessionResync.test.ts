import { describe, expect, it, vi } from 'vitest';
import { resyncSessions } from './sessionResync';
import { newSessionId, trackSession, untrackSession } from './engineProcess';
import type { EngineSessionKind, ResyncOutcome, TrackedSession } from './reconnect';

/**
 * 崩溃重连后的会话恢复（R3-19）。
 *
 * 此前 TrackedSession.kind 只接受 dir/text，picture/table/hex/merge/merge3
 * 从不登记 → 引擎崩溃后这些视图持失效句柄，后续请求全 410/411。
 */

const ALL_KINDS: EngineSessionKind[] = [
  'dir',
  'text',
  'merge',
  'merge3',
  'picture',
  'table',
  'hex',
];

let fakeSeq = 0;

function io(overrides: Partial<Parameters<typeof resyncSessions>[1]> = {}) {
  const emitted: ResyncOutcome[] = [];
  return {
    emitted,
    io: {
      resync: vi.fn(async () => {}),
      newSessionId: (kind: EngineSessionKind) => `${kind}-new-${++fakeSeq}`,
      emit: (o: ResyncOutcome) => emitted.push(o),
      ...overrides,
    },
  };
}

describe('resyncSessions：全部会话种类都要恢复', () => {
  it.each(ALL_KINDS)('%s 会话：resync 成功时句柄不变、kind 原样广播', async (kind) => {
    const tracked = new Map<string, TrackedSession>([['old-1', { kind, params: { left: '/L' } }]]);
    const { emitted, io: deps } = io();
    await resyncSessions(tracked, deps);

    expect(deps.resync).toHaveBeenCalledWith('old-1');
    expect(emitted).toEqual([
      { oldSessionId: 'old-1', newSessionId: 'old-1', kind, mode: 'resync' },
    ]);
    // resync 成功不换句柄，登记表保持原样
    expect([...tracked.keys()]).toEqual(['old-1']);
  });

  it.each(ALL_KINDS)('%s 会话：resync 失败时按同种类分配新句柄并重新登记', async (kind) => {
    const params = { left: '/L', right: '/R', rules: { ignoreWhitespace: true } };
    const tracked = new Map<string, TrackedSession>([['old-1', { kind, params }]]);
    const { emitted, io: deps } = io({
      resync: vi.fn(async () => {
        throw new Error('410');
      }),
    });
    await resyncSessions(tracked, deps);

    expect(emitted).toHaveLength(1);
    expect(emitted[0]).toMatchObject({ oldSessionId: 'old-1', kind, mode: 'replay' });
    const newId = emitted[0].newSessionId;
    expect(newId.startsWith(`${kind}-new-`)).toBe(true);
    expect(newId).not.toBe('old-1');
    expect(tracked.has('old-1')).toBe(false);
    // 新句柄必须重新登记且参数不丢（否则第二次崩溃不再恢复 / 重放口径漂移）
    expect(tracked.get(newId)).toEqual({ kind, params });
  });

  it('第二次崩溃：重放后的新句柄仍在登记表内，可再次恢复', async () => {
    const tracked = new Map<string, TrackedSession>([['old-1', { kind: 'hex', params: {} }]]);
    const first = io({
      resync: vi.fn(async () => {
        throw new Error('410');
      }),
    });
    await resyncSessions(tracked, first.io);
    const rebound = first.emitted[0].newSessionId;

    const second = io({
      resync: vi.fn(async () => {
        throw new Error('410');
      }),
    });
    await resyncSessions(tracked, second.io);
    const again = second.emitted[0].newSessionId;
    expect(second.emitted[0]).toMatchObject({
      oldSessionId: rebound,
      kind: 'hex',
      mode: 'replay',
    });
    expect(again).not.toBe(rebound);
    expect(tracked.has(rebound)).toBe(false);
    expect(tracked.has(again)).toBe(true);
  });

  it('单个会话 resync 失败不得中断其余会话的恢复', async () => {
    const tracked = new Map<string, TrackedSession>([
      ['a', { kind: 'dir', params: {} }],
      ['b', { kind: 'picture', params: {} }],
      ['c', { kind: 'table', params: {} }],
    ]);
    const { emitted, io: deps } = io({
      resync: vi.fn(async (id: string) => {
        if (id === 'b') throw new Error('-32601');
      }),
    });
    await resyncSessions(tracked, deps);

    expect(emitted.map((o) => [o.oldSessionId, o.mode])).toEqual([
      ['a', 'resync'],
      ['b', 'replay'],
      ['c', 'resync'],
    ]);
    expect(deps.resync).toHaveBeenCalledTimes(3);
  });

  it('按登记顺序恢复；空登记表不广播', async () => {
    const empty = io();
    await resyncSessions(new Map(), empty.io);
    expect(empty.emitted).toEqual([]);
    expect(empty.io.resync).not.toHaveBeenCalled();

    const tracked = new Map<string, TrackedSession>([
      ['z', { kind: 'text', params: {} }],
      ['a', { kind: 'merge3', params: {} }],
    ]);
    const { emitted, io: deps } = io();
    await resyncSessions(tracked, deps);
    expect(emitted.map((o) => o.oldSessionId)).toEqual(['z', 'a']);
  });
});

describe('trackSession / newSessionId：登记口径覆盖全部会话种类', () => {
  // 类型口径由 tsc -b 守：把 kind 收窄回 'dir'|'text' 时本文件编译失败
  it('七种会话都能生成对应前缀句柄并登记（此前类型只允许 dir/text）', () => {
    for (const kind of ALL_KINDS) {
      const id = newSessionId(kind);
      expect(id.startsWith(`${kind}-`)).toBe(true);
      trackSession(id, kind, { left: '/L', right: '/R' });
      untrackSession(id);
    }
  });

  it('同种类两次生成的句柄互不相同（重放不会撞句柄）', () => {
    const a = newSessionId('picture');
    const b = newSessionId('picture');
    expect(a).not.toBe(b);
  });
});
