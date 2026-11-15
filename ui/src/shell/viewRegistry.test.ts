import { describe, it, expect } from 'vitest';
import { createDefaultRegistry, createViewCtx, setNoticeSink, ViewRegistry } from './viewRegistry';
import type { SessionService } from '../session/service';

describe('shell/viewRegistry 视图插件注册表', () => {
  it('默认注册表：home + 八类会话类型，按 order 排序', () => {
    const reg = createDefaultRegistry();
    const list = reg.listByOrder();
    expect(list.map((p) => p.id)).toEqual([
      'home',
      'folder-compare',
      'text-compare',
      'folder-sync',
      'folder-merge',
      'table-compare',
      'hex-compare',
      'picture-compare',
      'text-merge',
    ]);
    expect(reg.get('home')?.title).toBe('主页');
  });

  it('未实现视图带阶段标注（主页置灰 tooltip 口径）', () => {
    const reg = createDefaultRegistry();
    const table = reg.get('table-compare')!;
    expect(table.implemented).toBe(true);
    const folder = reg.get('folder-compare')!;
    expect(folder.implemented).toBe(true);
    expect(folder.phaseNote).toBeUndefined();
  });

  it('能力声明：目录/文本双窗格；合并三窗格插件属后续阶段注册', () => {
    const reg = createDefaultRegistry();
    expect(reg.get('folder-compare')!.capabilities.panes).toBe(2);
    expect(reg.get('text-compare')!.capabilities.layoutSwitch).toBe(true);
    expect(reg.get('home')!.capabilities.editable).toBe(false);
  });

  it('implementedKinds 只返回已实现的会话类型（P8 起含同步/合并）', () => {
    const reg = createDefaultRegistry();
    expect(reg.implementedKinds()).toEqual([
      'folder-compare',
      'text-compare',
      'folder-sync',
      'folder-merge',
      'table-compare',
      'hex-compare',
      'picture-compare',
      'text-merge',
    ]);
    // P8 同步/合并视图能力声明：同步双窗格、合并三窗格，reports 面开放
    expect(reg.get('folder-sync')!.capabilities.reports).toBe(true);
    expect(reg.get('folder-merge')!.capabilities.panes).toBe(3);
    // 队列 3：文本合并三窗格 + 可编辑输出（FR-TMRG-001/005）
    expect(reg.get('text-merge')!.capabilities.panes).toBe(3);
    expect(reg.get('text-merge')!.capabilities.editable).toBe(true);
  });

  it('重复 id 注册抛错；unknown 查询返回 undefined', () => {
    const reg = new ViewRegistry();
    const base = {
      id: 'x' as never,
      order: 1,
      icon: '',
      title: 'x',
      implemented: true,
      capabilities: { panes: 2 as const, editable: false, reports: false, layoutSwitch: false },
      commands: [],
    };
    reg.register(base);
    expect(() => reg.register({ ...base })).toThrow(/重复/);
    expect(reg.get('nope' as never)).toBeUndefined();
  });

  it('createViewCtx：notice 经 sink 出口（AppShell 接状态栏）', () => {
    const seen: string[] = [];
    setNoticeSink((m) => seen.push(m));
    const ctx = createViewCtx({} as SessionService);
    ctx.notice('hello');
    expect(seen).toEqual(['hello']);
    setNoticeSink(null);
  });
});
