import { describe, expect, it } from 'vitest';
import {
  closeSession,
  hexBytes,
  merge3Open,
  merge3OpenIdempotent,
  openHex,
  openHexIdempotent,
  openPicture,
  openPictureIdempotent,
  openTable,
  openTableIdempotent,
  tableSections,
} from './engineProcess';

/**
 * R3-10 回归：切回标签 / StrictMode 双挂载导致的重复 open。
 *
 * 引擎的 sessions 表为全部会话种类共用一个命名空间，重复 sessionId 一律 411。
 * 此前 table/hex 视图直接调 openTable/openHex，重挂载即报"会话已存在"并整屏报错。
 *
 * 本测试经 engineProcess 单例打到进程内演示引擎（node 环境无 Tauri → mock），
 * 不用替身：411 由 mock 的真实会话表产生，close→reopen 的效果由后续数据 RPC 验证。
 */

const tableArgs = (sessionId: string) => ({
  sessionId,
  left: '/demo/a.csv',
  right: '/demo/b.csv',
  options: null,
});

const hexArgs = (sessionId: string) => ({
  sessionId,
  left: '/demo/a.bin',
  right: '/demo/b.bin',
  options: null,
});

const pictureArgs = (sessionId: string, tolerance = 8) => ({
  sessionId,
  left: '/demo/a.png',
  right: '/demo/b.png',
  options: { tolerance, block: 32 },
});

const merge3Args = (sessionId: string) => ({
  sessionId,
  base: '/demo/base.txt',
  left: '/demo/left.txt',
  right: '/demo/right.txt',
  options: null,
});

describe('重复 open 的 411 复现（R3-10）', () => {
  it('table/hex/picture/merge3：同句柄二次 open 一律被引擎拒绝', async () => {
    await openTable(tableArgs('dup-table'));
    await expect(openTable(tableArgs('dup-table'))).rejects.toMatchObject({ code: 411 });

    await openHex(hexArgs('dup-hex'));
    await expect(openHex(hexArgs('dup-hex'))).rejects.toMatchObject({ code: 411 });

    await openPicture(pictureArgs('dup-pic'));
    await expect(openPicture(pictureArgs('dup-pic'))).rejects.toMatchObject({ code: 411 });

    await merge3Open(merge3Args('dup-m3'));
    await expect(merge3Open(merge3Args('dup-m3'))).rejects.toMatchObject({ code: 411 });
  });

  it('句柄跨种类也冲突（引擎只有一张 sessions 表）', async () => {
    await openTable(tableArgs('cross-kind'));
    await expect(openHex(hexArgs('cross-kind'))).rejects.toMatchObject({ code: 411 });
  });
});

describe('幂等 open：411 → close → reopen', () => {
  it('openTableIdempotent：重挂载拿到与首次一致的统计与列', async () => {
    const args = tableArgs('idem-table');
    const first = await openTable(args);
    const again = await openTableIdempotent(args);
    expect(again.sessionId).toBe('idem-table');
    expect(again.rowCount).toBe(first.rowCount);
    expect(again.columns).toEqual(first.columns);
    expect(again.summary).toEqual(first.summary);
    // 重建后的会话必须真的能取数据（不是只返回了一个壳）
    const page = await tableSections({ sessionId: 'idem-table', offset: 0, count: 5 });
    expect(page.rows.length).toBeGreaterThan(0);
  });

  it('openHexIdempotent：重挂载拿到一致尺寸，且分页可用', async () => {
    const args = hexArgs('idem-hex');
    const first = await openHex(args);
    const again = await openHexIdempotent(args);
    expect(again.sessionId).toBe('idem-hex');
    expect(again.leftSize).toBe(first.leftSize);
    expect(again.rightSize).toBe(first.rightSize);
    expect(again.summary).toEqual(first.summary);
    const page = await hexBytes({ sessionId: 'idem-hex', side: 'left', offset: 0, length: 64 });
    expect(page.lines.length).toBe(4);
  });

  it('openPictureIdempotent：容差变化复用同句柄也能重比较', async () => {
    const first = await openPicture(pictureArgs('idem-pic', 8));
    const again = await openPictureIdempotent(pictureArgs('idem-pic', 32));
    expect(again.sessionId).toBe('idem-pic');
    expect(again.width).toBe(first.width);
    expect(again.blocks.length).toBeGreaterThan(0);
  });

  it('merge3OpenIdempotent：重挂载拿到一致的块统计', async () => {
    const args = merge3Args('idem-m3');
    const first = await merge3Open(args);
    const again = await merge3OpenIdempotent(args);
    expect(again.sessionId).toBe('idem-m3');
    expect(again.chunkCount).toBe(first.chunkCount);
    expect(again.conflictCount).toBe(first.conflictCount);
  });

  it('首次 open（无 411）不受影响：句柄与结果照常', async () => {
    const fresh = await openTableIdempotent(tableArgs('idem-fresh'));
    expect(fresh.sessionId).toBe('idem-fresh');
    expect(fresh.rowCount).toBeGreaterThan(0);
  });

  it('真的执行了 close：旧种类会话被释放，句柄改由新种类持有', async () => {
    await openTable(tableArgs('steal-handle'));
    await openHexIdempotent(hexArgs('steal-handle'));
    // 表格会话已被 close 删除 → 410；十六进制会话可用
    await expect(
      tableSections({ sessionId: 'steal-handle', offset: 0, count: 5 }),
    ).rejects.toMatchObject({ code: 410 });
    const page = await hexBytes({
      sessionId: 'steal-handle',
      side: 'right',
      offset: 0,
      length: 16,
    });
    expect(page.lines.length).toBe(1);
  });

  it('非 411 错误原样抛出，不得当成"会话已存在"重试', async () => {
    await expect(openTableIdempotent(tableArgs(''))).rejects.toMatchObject({ code: -32602 });
    await expect(openHexIdempotent(hexArgs(''))).rejects.toMatchObject({ code: -32602 });
    await expect(openPictureIdempotent(pictureArgs(''))).rejects.toMatchObject({ code: -32602 });
    await expect(merge3OpenIdempotent(merge3Args(''))).rejects.toMatchObject({ code: -32602 });
  });
});

describe('演示引擎会话表保真（R3-10 的可测前提）', () => {
  it('session/close 释放 picture/table/hex 会话：关闭后同句柄可重新 open', async () => {
    await openTable(tableArgs('close-table'));
    await openHex(hexArgs('close-hex'));
    await openPicture(pictureArgs('close-pic'));
    await closeSession({ sessionId: 'close-table' });
    await closeSession({ sessionId: 'close-hex' });
    await closeSession({ sessionId: 'close-pic' });
    // 未修复前 close 不删这三张表 → 重新 open 仍 411
    await expect(openTable(tableArgs('close-table'))).resolves.toMatchObject({
      sessionId: 'close-table',
    });
    await expect(openHex(hexArgs('close-hex'))).resolves.toMatchObject({ sessionId: 'close-hex' });
    await expect(openPicture(pictureArgs('close-pic'))).resolves.toMatchObject({
      sessionId: 'close-pic',
    });
  });

  it('close 幂等：不存在的句柄返回 null', async () => {
    await expect(closeSession({ sessionId: 'never-opened' })).resolves.toBeNull();
  });

  it('dir/mergeScan 同会话重扫描仍幂等（不得被全局唯一守卫误伤）', async () => {
    const { mergeScan } = await import('./engineProcess');
    const args = {
      sessionId: 'rescan-merge',
      base: '/demo/base',
      left: '/demo/l',
      right: '/demo/r',
    };
    const first = await mergeScan(args);
    const again = await mergeScan(args);
    expect(again.rows.length).toBe(first.rows.length);
  });
});
