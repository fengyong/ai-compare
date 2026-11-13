import { describe, it, expect } from 'vitest';
import { MAX_AUTO_RESTARTS, ReconnectPolicy } from './reconnect';

describe('ipc/reconnect 横幅状态机', () => {
  it('崩溃 → 自动重启一次 → 恢复完成 → ok', () => {
    const p = new ReconnectPolicy();
    const seen: string[] = [];
    p.subscribe((ph) => seen.push(ph));

    expect(p.getPhase()).toBe('ok');
    p.onDisconnect();
    expect(p.getPhase()).toBe('reconnecting');
    expect(p.canAutoRestart()).toBe(true);
    p.beginRestart();
    p.onConnected();
    expect(p.getPhase()).toBe('resyncing');
    p.onResynced();
    expect(p.getPhase()).toBe('ok');
    expect(seen).toEqual(['reconnecting', 'resyncing', 'ok']);
  });

  it('自动重启仅一次：第二次断线不再自动重启（failed）', () => {
    const p = new ReconnectPolicy();
    p.onDisconnect();
    p.beginRestart();
    p.onConnected();
    p.onResynced();
    // 第二次崩溃：计数已重置为 0（成功恢复后清零，允许再次自动重启）
    p.onDisconnect();
    expect(p.canAutoRestart()).toBe(true);
    p.beginRestart();
    expect(p.canAutoRestart()).toBe(false);
    p.onFailed();
    expect(p.getPhase()).toBe('failed');
  });

  it('同一崩溃脉冲内重复 onDisconnect 去重（不反复进 reconnecting）', () => {
    const p = new ReconnectPolicy();
    p.onDisconnect();
    p.onDisconnect();
    expect(p.getPhase()).toBe('reconnecting');
    // 期间 canAutoRestart 仍为首次
    expect(p.canAutoRestart()).toBe(true);
    p.beginRestart();
    expect(p.canAutoRestart()).toBe(false);
  });

  it('重启失败 → failed；手动重试清计数回 reconnecting', () => {
    const p = new ReconnectPolicy();
    p.onDisconnect();
    p.beginRestart();
    p.onFailed();
    expect(p.getPhase()).toBe('failed');
    expect(p.canAutoRestart()).toBe(false);
    p.manualRetry();
    expect(p.getPhase()).toBe('reconnecting');
    expect(p.canAutoRestart()).toBe(true);
  });

  it('resync 完成清零重启计数；settled 切终态', () => {
    const p = new ReconnectPolicy();
    p.onDisconnect();
    p.beginRestart();
    p.onResynced();
    expect(p.canAutoRestart()).toBe(true);
    p.settled('degraded');
    expect(p.getPhase()).toBe('degraded');
    p.settled('ok');
    expect(p.getPhase()).toBe('ok');
  });

  it('MAX_AUTO_RESTARTS = 1（任务口径：崩溃后自动重启一次）', () => {
    expect(MAX_AUTO_RESTARTS).toBe(1);
  });
});
