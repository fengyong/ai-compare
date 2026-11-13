import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DirDelta, DirRow } from '../../protocol/types';
import { DirRowCache } from './dirRowCache';

/** 目录行缓存：dir/delta 合并语义（upsert 按行 id 幂等 / removals / stats） */
const row = (id: string, over: Partial<DirRow> = {}): DirRow => ({
  id,
  name: id.split('/').pop() ?? id,
  kind: 'file',
  left: { size: 1, mtimeMs: 0 },
  right: { size: 1, mtimeMs: 0 },
  status: 'same',
  hasChildren: false,
  ...over,
});

const delta = (over: Partial<DirDelta> & Pick<DirDelta, 'upserts'>): DirDelta => ({
  sessionId: 's1',
  rev: 1n,
  removals: [],
  stats: {
    total: over.upserts.length,
    same: over.upserts.length,
    quickDiff: 0,
    contentDiff: 0,
    leftOnly: 0,
    rightOnly: 0,
    dirSame: 0,
  },
  progress: { scanned: over.upserts.length, total: over.upserts.length },
  batch: { index: 0, hasMore: false },
  ...over,
});

describe('DirRowCache', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('按到达顺序追加 delta 行，total 随行数增长', () => {
    const c = new DirRowCache('s1');
    c.applyDelta(delta({ upserts: [row('a'), row('b')] }));
    c.applyDelta(delta({ upserts: [row('c')] }));
    expect(c.getRow(0)?.id).toBe('a');
    expect(c.getRow(2)?.id).toBe('c');
    expect(c.getStats().total).toBe(3);
  });

  it('upsert 按行 id 幂等更新（不追加重复行）', () => {
    const c = new DirRowCache('s1');
    c.applyDelta(delta({ upserts: [row('a'), row('b')] }));
    c.applyDelta(delta({ upserts: [row('a', { status: 'contentDiff' })] }));
    expect(c.getStats().total).toBe(2);
    expect(c.getRow(0)?.status).toBe('contentDiff');
    expect(c.getRow(0)?.name).toBe('a');
  });

  it('removals 按行 id 删除并重排索引', () => {
    const c = new DirRowCache('s1');
    c.applyDelta(delta({ upserts: [row('a'), row('b'), row('c')] }));
    c.applyDelta(delta({ upserts: [], removals: ['b'] }));
    expect(c.getStats().total).toBe(2);
    expect(c.getRow(0)?.id).toBe('a');
    expect(c.getRow(1)?.id).toBe('c');
    // 删除后再 upsert 同 id 走追加
    c.applyDelta(delta({ upserts: [row('b')] }));
    expect(c.getRow(2)?.id).toBe('b');
  });

  it('stats / progress 随 delta 更新，version 递增触发订阅', () => {
    const c = new DirRowCache('s1');
    const versions: number[] = [];
    c.subscribe(() => versions.push(c.getSnapshot()));
    c.applyDelta(
      delta({
        upserts: [row('a'), row('b', { status: 'leftOnly' })],
        stats: {
          total: 2,
          same: 1,
          quickDiff: 0,
          contentDiff: 0,
          leftOnly: 1,
          rightOnly: 0,
          dirSame: 0,
        },
        progress: { scanned: 2, total: 2 },
      })
    );
    const snap = c.getStats();
    expect(snap.stats.leftOnly).toBe(1);
    expect(snap.progress).toEqual({ scanned: 2, total: 2 });
    expect(snap.scanState).toBe('running'); // 未见 open 结果前保持 running
    expect(versions.length).toBe(1);
  });

  it('applyOpenResult 记录扫描终态；ensureOpened 幂等（只 open 一次）', async () => {
    const c = new DirRowCache('s1');
    const open = vi.fn().mockResolvedValue({ rowCount: 5, scanState: 'completed' as const });
    await Promise.all([c.ensureOpened(open), c.ensureOpened(open)]);
    expect(open).toHaveBeenCalledTimes(1);
    expect(c.getStats().scanState).toBe('completed');
  });

  it('ensureOpened 失败后允许重试', async () => {
    const c = new DirRowCache('s1');
    const open = vi
      .fn()
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValue({ rowCount: 0, scanState: 'completed' as const });
    await expect(c.ensureOpened(open)).rejects.toThrow('boom');
    await expect(c.ensureOpened(open)).resolves.toBeUndefined();
    expect(open).toHaveBeenCalledTimes(2);
  });
});
