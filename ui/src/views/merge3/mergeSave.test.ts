import { describe, expect, it } from 'vitest';
import { runMergeSave, type MergeSaveIO, type MergeSaveOptions } from './mergeSave';
import type { MergeSaveParams, MergeSaveResult } from '../../protocol/types';

/**
 * 合并输出落盘控制器（R3-20，FR-TMRG-006）：覆盖警示先于写入、
 * 取消不得落盘、能力缺失与失败必须可见、backup/mode 透传引擎。
 */

const opts: MergeSaveOptions = { mode: 'plain', markerStyle: 'merge', backup: true };

const result = (path: string): MergeSaveResult => ({
  sessionId: 's1',
  path,
  bytes: 12 as unknown as bigint,
  lineCount: 3,
  backupPath: `${path}.bak`,
  conflictCount: 2,
  unresolvedChunks: [],
  hasConflicts: false,
  rev: 1 as unknown as bigint,
});

function recorder(impl?: (p: MergeSaveParams) => Promise<MergeSaveResult | null>) {
  const calls: MergeSaveParams[] = [];
  const confirmed: string[] = [];
  const order: string[] = [];
  const io: MergeSaveIO = {
    confirmOverwrite: async (path) => {
      confirmed.push(path);
      order.push('confirm');
      return true;
    },
    save: async (p) => {
      calls.push(p);
      order.push('save');
      if (impl) return impl(p);
      return result(p.path);
    },
  };
  return { calls, confirmed, order, io };
}

describe('runMergeSave：覆盖警示与写入顺序', () => {
  it('先警示后写入，目标路径 trim 后传给引擎', async () => {
    const { calls, confirmed, order, io } = recorder();
    const out = await runMergeSave('s1', '  /tmp/out.txt  ', opts, io);
    expect(order).toEqual(['confirm', 'save']);
    expect(confirmed).toEqual(['/tmp/out.txt']);
    expect(calls).toEqual([
      { sessionId: 's1', path: '/tmp/out.txt', mode: 'plain', markerStyle: 'merge', backup: true },
    ]);
    expect(out.result?.path).toBe('/tmp/out.txt');
    expect(out).toMatchObject({ unsupported: false, cancelled: false, error: null });
  });

  it('用户取消：不发 save，cancelled=true', async () => {
    const { calls, io } = recorder();
    const cancelIO: MergeSaveIO = { ...io, confirmOverwrite: async () => false };
    const out = await runMergeSave('s1', '/tmp/out.txt', opts, cancelIO);
    expect(calls).toEqual([]);
    expect(out).toEqual({ result: null, unsupported: false, cancelled: true, error: null });
  });

  it('marker 模式与 backup=false（/nobackups）原样透传', async () => {
    const { calls, io } = recorder();
    await runMergeSave(
      's1',
      '/tmp/out.txt',
      { mode: 'marker', markerStyle: 'diff3', backup: false },
      io,
    );
    expect(calls[0]).toMatchObject({ mode: 'marker', markerStyle: 'diff3', backup: false });
  });
});

describe('runMergeSave：失败与能力缺失不得被当成已保存', () => {
  it('引擎无 mergeSave 能力（null）→ unsupported，result 为空', async () => {
    const { io } = recorder(async () => null);
    const out = await runMergeSave('s1', '/tmp/out.txt', opts, io);
    expect(out).toEqual({ result: null, unsupported: true, cancelled: false, error: null });
  });

  it('引擎报错 → error 为消息，不谎报成功', async () => {
    const { io } = recorder(async () => {
      throw new Error('磁盘写入失败: /tmp/out.txt');
    });
    const out = await runMergeSave('s1', '/tmp/out.txt', opts, io);
    expect(out).toEqual({
      result: null,
      unsupported: false,
      cancelled: false,
      error: '磁盘写入失败: /tmp/out.txt',
    });
  });

  it('非 Error 抛出物也转成可读消息', async () => {
    const { io } = recorder(async () => {
      throw 'boom';
    });
    const out = await runMergeSave('s1', '/tmp/out.txt', opts, io);
    expect(out.error).toBe('boom');
  });

  it('成功时保留备份路径与未决冲突清单（供调用方提示）', async () => {
    const { io } = recorder(async (p) => ({
      ...result(p.path),
      hasConflicts: true,
      unresolvedChunks: [1, 3],
    }));
    const out = await runMergeSave('s1', '/tmp/out.txt', opts, io);
    expect(out.result?.backupPath).toBe('/tmp/out.txt.bak');
    expect(out.result?.unresolvedChunks).toEqual([1, 3]);
    expect(out.result?.hasConflicts).toBe(true);
  });
});
