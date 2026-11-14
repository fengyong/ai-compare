import { describe, expect, it } from 'vitest';
import { sessionKindOfTab, specsOfTab, resolveMergeBase } from './sessionSpecs';
import type { Tab } from './shellState';
import { SESSION_KINDS } from '../session/model';

/**
 * 标签 → 命名会话规格（R3-08）。
 *
 * 此前 merge/merge3 只落 `{left: base, right}`（真实 left 丢失），
 * picture/table/hex 落空串且被另存为标成 folder-compare → 重开必然错类型/错路径。
 */

const tab = (kind: Tab['kind'], data: Partial<Tab> = {}): Tab =>
  ({ id: 't', kind, title: 'x', ...data }) as Tab;

describe('sessionKindOfTab：标签类型 → 会话类型', () => {
  it('九种标签类型逐一映射（home 无会话语义，兜底目录比较）', () => {
    expect(sessionKindOfTab('folder')).toBe('folder-compare');
    expect(sessionKindOfTab('text')).toBe('text-compare');
    expect(sessionKindOfTab('sync')).toBe('folder-sync');
    expect(sessionKindOfTab('merge')).toBe('folder-merge');
    expect(sessionKindOfTab('merge3')).toBe('text-merge');
    expect(sessionKindOfTab('picture')).toBe('picture-compare');
    expect(sessionKindOfTab('table')).toBe('table-compare');
    expect(sessionKindOfTab('hex')).toBe('hex-compare');
    expect(sessionKindOfTab('home')).toBe('folder-compare');
  });

  it('映射结果必须是合法会话类型（会话树/设置作用域据此分派）', () => {
    const kinds: Tab['kind'][] = [
      'home',
      'folder',
      'text',
      'sync',
      'merge',
      'merge3',
      'picture',
      'table',
      'hex',
    ];
    for (const k of kinds) expect(SESSION_KINDS).toContain(sessionKindOfTab(k));
  });
});

describe('specsOfTab：标签 → 会话规格', () => {
  it('两路会话落真实 left/right', () => {
    expect(
      specsOfTab(tab('folder', { folder: { left: '/L', right: '/R', sessionId: 's' } })),
    ).toEqual({
      left: '/L',
      right: '/R',
    });
    expect(specsOfTab(tab('sync', { sync: { left: '/L', right: '/R', sessionId: 's' } }))).toEqual({
      left: '/L',
      right: '/R',
    });
    expect(
      specsOfTab(
        tab('text', {
          text: {
            sessionId: 's',
            left: { path: '/L.txt', name: 'L.txt' },
            right: { path: '/R.txt', name: 'R.txt' },
          },
        }),
      ),
    ).toEqual({ left: '/L.txt', right: '/R.txt' });
  });

  it('图片/表格/十六进制落真实路径（此前落空串 → 重开无内容）', () => {
    expect(
      specsOfTab(tab('picture', { picture: { left: '/a.png', right: '/b.png', sessionId: 's' } })),
    ).toEqual({
      left: '/a.png',
      right: '/b.png',
    });
    expect(
      specsOfTab(tab('table', { table: { left: '/a.csv', right: '/b.csv', sessionId: 's' } })),
    ).toEqual({
      left: '/a.csv',
      right: '/b.csv',
    });
    expect(
      specsOfTab(tab('hex', { hex: { left: '/a.bin', right: '/b.bin', sessionId: 's' } })),
    ).toEqual({
      left: '/a.bin',
      right: '/b.bin',
    });
  });

  it('三路会话 base/left/right 三个路径位都要落盘（此前 left 被 base 顶替）', () => {
    expect(
      specsOfTab(tab('merge', { merge: { base: '/B', left: '/L', right: '/R', sessionId: 's' } })),
    ).toEqual({ base: '/B', left: '/L', right: '/R' });
    expect(
      specsOfTab(
        tab('merge3', {
          merge3: { base: '/B.txt', left: '/L.txt', right: '/R.txt', sessionId: 's' },
        }),
      ),
    ).toEqual({ base: '/B.txt', left: '/L.txt', right: '/R.txt' });
  });

  it('三路会话的 base 与 left 不同才可区分（回归锚点：base===left 即旧缺陷）', () => {
    const specs = specsOfTab(
      tab('merge3', { merge3: { base: '/B', left: '/L', right: '/R', sessionId: 's' } }),
    );
    expect(specs.base).not.toBe(specs.left);
  });

  it('无数据标签落空串（保存入口据此拒绝，不写半截会话）', () => {
    expect(specsOfTab(tab('home'))).toEqual({ left: '', right: '' });
    expect(specsOfTab(tab('folder'))).toEqual({ left: '', right: '' });
  });
});

describe('resolveMergeBase：三路会话的基准路径', () => {
  it('显式 base 优先（主页 folder-merge 此前忽略「基准路径」输入，静默用 left 顶替）', () => {
    expect(resolveMergeBase('/B', '/L')).toBe('/B');
    expect(resolveMergeBase('  /B  ', '/L')).toBe('/B');
  });

  it('base 留空/全空白时以 left 兼作（BC 允许 base 缺省）', () => {
    expect(resolveMergeBase('', '/L')).toBe('/L');
    expect(resolveMergeBase('   ', '/L')).toBe('/L');
  });

  it('base 与 left 都空 → 空串（由入口的路径必填校验拦下）', () => {
    expect(resolveMergeBase('', '')).toBe('');
  });
});
