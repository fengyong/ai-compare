import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DirDelta, DirRow } from '../../protocol/types';
import { DirRowCache, type DirViewState } from './dirRowCache';
import { emptySelection } from './dirSelection';

/** 目录行缓存：dir/delta 合并语义（upsert 按行 id 幂等 / removals / stats） */
const row = (id: string, over: Partial<DirRow> = {}): DirRow => ({
  id,
  name: id.split('/').pop() ?? id,
  kind: 'file',
  left: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 },
  right: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 },
  status: 'same',
  hasChildren: false,
  hidden: false,
  suppressed: false,
  leftPath: id,
  rightPath: id,
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
    dirDiff: 0,
    similar: 0,
  },
  progress: { scanned: over.upserts.length, total: over.upserts.length },
  batch: { index: 0, hasMore: false },
  scanErrors: [],
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
          dirDiff: 0,
          similar: 0,
        },
        progress: { scanned: 2, total: 2 },
      }),
    );
    const snap = c.getStats();
    expect(snap.stats.leftOnly).toBe(1);
    expect(snap.progress).toEqual({ scanned: 2, total: 2 });
    expect(snap.scanState).toBe('running'); // 未见 open 结果前保持 running
    expect(versions.length).toBe(1);
  });

  it('引擎省略 0 值计数器（skip_serializing_if）时 UI 读到 0 而非 undefined', () => {
    const c = new DirRowCache('s1');
    const d = delta({ upserts: [row('a')] });
    // 线上载荷确实没有 similar/dirDiff 两键（引擎对 0 值省略），故按线上形状构造
    const wire = {
      ...d,
      stats: {
        total: 1,
        same: 1,
        quickDiff: 0,
        contentDiff: 0,
        leftOnly: 0,
        rightOnly: 0,
        dirSame: 0,
      },
    } as unknown as DirDelta;
    c.applyDelta(wire);
    expect(c.getStats().stats.similar).toBe(0);
    expect(c.getStats().stats.dirDiff).toBe(0);

    c.applyDelta(delta({ upserts: [row('a')], stats: { ...d.stats!, similar: 2, dirDiff: 1 } }));
    expect(c.getStats().stats.similar).toBe(2);
    expect(c.getStats().stats.dirDiff).toBe(1);
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

/** P6b：标记集会话缓存层（空格标记持久于会话；变更推版本号） */
describe('DirRowCache marks', () => {
  it('toggleMark/setMarks 持久于缓存并推版本号', () => {
    const c = new DirRowCache('s1');
    let v0 = c.getSnapshot();
    expect(c.toggleMark('a.txt')).toBe(true);
    expect(c.isMarked('a.txt')).toBe(true);
    expect(c.getMarkCount()).toBe(1);
    expect(c.getSnapshot()).toBe(v0 + 1); // 版本号推进触发视图重渲染

    c.setMarks(['b.txt', 'c.txt'], true);
    expect(c.getMarkCount()).toBe(3);
    c.setMarks(['a.txt', 'b.txt'], false);
    expect(c.isMarked('c.txt')).toBe(true);
    expect(c.getMarkCount()).toBe(1);
    expect(c.getMarks().has('c.txt')).toBe(true);
    v0 = c.getSnapshot();
    c.setMarks(['zz.txt'], false); // 无变化不推版本
    expect(c.getSnapshot()).toBe(v0);
  });

  it('delta removals 同步清标记（防悬挂 id）', () => {
    const c = new DirRowCache('s1');
    c.applyDelta({
      sessionId: 's1',
      upserts: [row('a.txt'), row('b.txt')],
      removals: [],
      stats: null,
      progress: null,
    } as unknown as DirDelta);
    c.toggleMark('a.txt');
    c.toggleMark('b.txt');
    c.applyDelta({
      sessionId: 's1',
      upserts: [],
      removals: ['a.txt'],
      stats: null,
      progress: null,
    } as unknown as DirDelta);
    expect(c.isMarked('a.txt')).toBe(false);
    expect(c.isMarked('b.txt')).toBe(true);
    expect(c.getMarkCount()).toBe(1);
  });
});

/** R3-09：读不到的子树必须可见（引擎经 dir/delta.scanErrors 回传，行状态置未知） */
describe('DirRowCache scanErrors', () => {
  it('首批错误进入快照；后续空批次不清掉既有错误', () => {
    const c = new DirRowCache('s1');
    expect(c.getStats().scanErrors).toEqual([]);
    c.applyDelta(
      delta({
        upserts: [row('a.txt')],
        scanErrors: [{ path: 'sub', message: 'Permission denied' }],
      }),
    );
    expect(c.getStats().scanErrors).toEqual([{ path: 'sub', message: 'Permission denied' }]);
    c.applyDelta(delta({ upserts: [row('b.txt')], batch: { index: 1, hasMore: false } }));
    expect(c.getStats().scanErrors).toEqual([{ path: 'sub', message: 'Permission denied' }]);
  });

  it('重扫首批为全量：同路径消息覆盖，已恢复的路径不再报错', () => {
    const c = new DirRowCache('s1');
    c.applyDelta(
      delta({
        upserts: [],
        scanErrors: [
          { path: 'sub', message: 'EACCES' },
          { path: 'gone', message: 'I/O error' },
        ],
      }),
    );
    c.applyDelta(delta({ upserts: [], scanErrors: [{ path: 'sub', message: 'EIO' }] }));
    expect(c.getStats().scanErrors).toEqual([{ path: 'sub', message: 'EIO' }]);
    c.applyDelta(delta({ upserts: [] }));
    expect(c.getStats().scanErrors).toEqual([]);
  });
});

/**
 * R3-28：视图偏好挂会话缓存层（切换标签卸载视图组件，但缓存存活，重挂据此 rehydrate）。
 * 仅视图偏好，不含瞬态对话框；不改行数据，故不推版本号（避免无谓重渲染）。
 */
describe('DirRowCache viewState', () => {
  const viewState = (over: Partial<DirViewState> = {}): DirViewState => ({
    expanded: new Set(['sub']),
    flat: true,
    suppress: false,
    displayPreset: 'all',
    filterText: '*.rs',
    appliedFilterText: '*.rs',
    sortSpec: { key: 'name', dir: 1 },
    markOnly: false,
    paneSel: { left: emptySelection, right: emptySelection },
    activeSide: 'right',
    ...over,
  });

  it('首次为 null（视图据默认值初始化）', () => {
    expect(new DirRowCache('s1').getViewState()).toBeNull();
  });

  it('setViewState/getViewState 往返保留全部偏好字段', () => {
    const c = new DirRowCache('s1');
    const vs = viewState();
    c.setViewState(vs);
    const got = c.getViewState();
    expect(got).toBe(vs); // 同一引用：视图重挂直接读回
    expect(got?.flat).toBe(true);
    expect(got?.expanded.has('sub')).toBe(true);
    expect(got?.appliedFilterText).toBe('*.rs');
    expect(got?.sortSpec).toEqual({ key: 'name', dir: 1 });
    expect(got?.activeSide).toBe('right');
  });

  it('再次写入覆盖旧值（最新偏好生效）', () => {
    const c = new DirRowCache('s1');
    c.setViewState(viewState({ flat: true, markOnly: false }));
    c.setViewState(viewState({ flat: false, markOnly: true }));
    expect(c.getViewState()?.flat).toBe(false);
    expect(c.getViewState()?.markOnly).toBe(true);
  });

  it('写视图偏好不推版本号（不改行数据，避免无谓重渲染）', () => {
    const c = new DirRowCache('s1');
    const versions: number[] = [];
    c.subscribe(() => versions.push(c.getSnapshot()));
    const v0 = c.getSnapshot();
    c.setViewState(viewState());
    c.setViewState(viewState({ flat: false }));
    expect(c.getSnapshot()).toBe(v0);
    expect(versions.length).toBe(0);
  });
});
