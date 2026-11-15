import { describe, it, expect } from 'vitest';
import { parentRescanParams } from './parentWriteback';

describe('parentWriteback（R3-17：子会话保存 → 父目录行回传）', () => {
  it('独立文本比较（无父会话）不触发任何重扫', () => {
    expect(parentRescanParams(undefined)).toBeNull();
    expect(parentRescanParams(null)).toBeNull();
    expect(parentRescanParams('')).toBeNull();
  });

  it('有父会话时重扫的是父会话句柄，且只要求 rescan', () => {
    const p = parentRescanParams('dir-parent-1');
    expect(p).not.toBeNull();
    expect(p!.sessionId).toBe('dir-parent-1');
    expect(p!.rescan).toBe(true);
  });

  it('过滤器/显示/抑制/平铺一律不传：不得覆盖父会话现有设置', () => {
    const p = parentRescanParams('dir-parent-1')!;
    // 引擎 dir/filter 按 is_some 门控：不传 = 不改动。子会话无从得知父会话的
    // 过滤器与显示设置，传具体值会把它们悄悄重置。
    // R3-21：这些字段在协议里是可选字段（缺键 = 不改动），不再是显式 null。
    expect(p.nameFilter).toBeUndefined();
    expect(p.otherFilter).toBeUndefined();
    expect(p.displayFilter).toBeUndefined();
    expect(p.suppress).toBeUndefined();
    expect(p.flat).toBeUndefined();
  });
});
