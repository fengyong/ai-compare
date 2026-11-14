import { describe, it, expect } from 'vitest';
import {
  KIND_META,
  ParseError,
  SESSION_KINDS,
  emptySessionsDoc,
  isSessionKind,
  newSessionId,
  parseKindDefaultsDoc,
  parseSessionsDoc,
  parseStateDoc,
  serializeDoc,
} from './model';

describe('session/model', () => {
  it('会话类型齐备且元信息标注阶段（队列 3 起八类）', () => {
    expect(SESSION_KINDS).toHaveLength(8);
    for (const k of SESSION_KINDS) {
      expect(KIND_META[k].title).toBeTruthy();
      expect(KIND_META[k].phase).toMatch(/^P\d/);
    }
    expect(KIND_META['folder-compare'].implemented).toBe(true);
    expect(KIND_META['table-compare'].implemented).toBe(true);
    // P8：同步/合并视图接入后置为已实现
    expect(KIND_META['folder-sync'].implemented).toBe(true);
    expect(KIND_META['folder-merge'].implemented).toBe(true);
    expect(KIND_META['table-compare'].implemented).toBe(true);
    // 队列 3：三向文本合并视图接入后置为已实现
    expect(KIND_META['text-merge'].implemented).toBe(true);
    expect(SESSION_KINDS).toContain('text-merge');
  });

  it('isSessionKind 判定', () => {
    expect(isSessionKind('folder-compare')).toBe(true);
    expect(isSessionKind('registry-compare')).toBe(false);
    expect(isSessionKind(42)).toBe(false);
  });

  it('newSessionId 唯一且带 s- 前缀', () => {
    const a = newSessionId();
    const b = newSessionId();
    expect(a).toMatch(/^s-/);
    expect(a).not.toBe(b);
  });

  it('parseSessionsDoc 完整文档往返', () => {
    const doc = emptySessionsDoc();
    doc.sessions.push({
      id: 's-1',
      kind: 'folder-compare',
      name: '发布目录对比',
      locked: true,
      createdAt: '2026-11-14T00:00:00.000Z',
      updatedAt: '2026-11-14T00:00:00.000Z',
      specs: { left: '/tmp/L', right: '/tmp/R' },
      overrides: { compare: { mtimeToleranceSec: 5 } },
      state: { displayFilter: ['diff'] },
      folderId: null,
    });
    const parsed = parseSessionsDoc(serializeDoc(doc));
    expect(parsed.sessions).toHaveLength(1);
    const s = parsed.sessions[0];
    expect(s.name).toBe('发布目录对比');
    expect(s.locked).toBe(true);
    expect(s.specs).toEqual({ left: '/tmp/L', right: '/tmp/R' });
    expect(s.overrides).toEqual({ compare: { mtimeToleranceSec: 5 } });
    expect(s.state.displayFilter).toEqual(['diff']);
  });

  it('parseSessionsDoc 容错：字段缺失按默认补齐', () => {
    const parsed = parseSessionsDoc(
      JSON.stringify({
        version: 1,
        folders: [{ id: 'f1', name: '工作' }],
        sessions: [{ id: 's1', kind: 'text-compare' }],
      })
    );
    expect(parsed.folders[0].children).toEqual([]);
    const s = parsed.sessions[0];
    expect(s.name).toBe('未命名会话');
    expect(s.locked).toBe(false);
    expect(s.specs).toEqual({ left: '', right: '' });
    expect(s.overrides).toEqual({});
    // 兜底：folderId 指向不存在的文件夹 → 置 null；无 rootFolderIds → 推导根
    expect(s.folderId).toBeNull();
    expect(parsed.rootFolderIds).toContain('f1');
  });

  it('parseSessionsDoc 非法输入抛 ParseError（供 .bak 回滚判定）', () => {
    expect(() => parseSessionsDoc('not json {')).toThrow(ParseError);
    expect(() => parseSessionsDoc('[]')).toThrow(ParseError);
    expect(() => parseSessionsDoc('{"version": 99}')).toThrow(ParseError);
  });

  it('parseSessionsDoc 丢弃未知 kind 的会话节点（前向兼容）', () => {
    const parsed = parseSessionsDoc(
      JSON.stringify({ version: 1, sessions: [{ id: 'x', kind: 'future-kind' }, { id: 'y', kind: 'folder-compare' }] })
    );
    expect(parsed.sessions.map((s) => s.id)).toEqual(['y']);
  });

  it('parseStateDoc：MRU 结构容错与默认值', () => {
    const good = parseStateDoc(
      JSON.stringify({
        version: 1,
        mru: { sessions: ['s-1'], paths: { 'folder-compare': { left: ['/a'], right: ['/b'] } }, max: 5 },
        ui: { showSessionPanel: false },
      })
    );
    expect(good.mru.max).toBe(5);
    expect(good.mru.paths['folder-compare']).toEqual({ left: ['/a'], right: ['/b'] });
    expect(good.ui.showSessionPanel).toBe(false);

    const fallback = parseStateDoc(JSON.stringify({ version: 1 }));
    expect(fallback.mru.max).toBe(10);
    expect(fallback.mru.sessions).toEqual([]);
    expect(fallback.ui.showSessionPanel).toBe(true);
    expect(() => parseStateDoc('{')).toThrow(ParseError);
  });

  it('parseKindDefaultsDoc：只收已知 kind', () => {
    const doc = parseKindDefaultsDoc(
      JSON.stringify({ version: 1, kinds: { 'folder-compare': { compare: { mtimeToleranceSec: 1 } }, bogus: {} } })
    );
    expect(doc.kinds['folder-compare']).toEqual({ compare: { mtimeToleranceSec: 1 } });
    expect(doc.kinds['bogus' as never]).toBeUndefined();
  });
});

describe('session/model：三路会话的 base 路径位（R3-08）', () => {
  const doc = (specs: unknown) =>
    JSON.stringify({
      version: 1,
      sessions: [{ id: 's1', kind: 'folder-merge', name: '三路', specs }],
    });

  it('base 与 left/right 一起往返（重开时 base 不再被 left 顶替）', () => {
    const parsed = parseSessionsDoc(doc({ base: '/B', left: '/L', right: '/R' }));
    expect(parsed.sessions[0].specs).toEqual({ base: '/B', left: '/L', right: '/R' });
  });

  it('两路会话不写 base：解析后不得凭空补出 base', () => {
    const parsed = parseSessionsDoc(doc({ left: '/L', right: '/R' }));
    expect(parsed.sessions[0].specs).toEqual({ left: '/L', right: '/R' });
    expect(parsed.sessions[0].specs.base).toBeUndefined();
  });

  it('base 为空串/非字符串按缺省处理（旧文件与手改文件容错）', () => {
    for (const base of ['', 42, null]) {
      const parsed = parseSessionsDoc(doc({ base, left: '/L', right: '/R' }));
      expect(parsed.sessions[0].specs.base).toBeUndefined();
    }
  });
});
