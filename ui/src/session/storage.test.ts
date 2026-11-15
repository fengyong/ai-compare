import { describe, it, expect } from 'vitest';
import { ParseError, parseSessionsDoc, serializeDoc, emptySessionsDoc } from './model';
import {
  createAtomicWritePlan,
  pushMru,
  recoverWithBackup,
  SETTINGS_FILES,
  normalizePathInput,
} from './storage';

describe('session/storage', () => {
  it('SETTINGS_FILES 固定四个文件名', () => {
    expect(SETTINGS_FILES).toEqual({
      sessions: 'sessions.json',
      kindDefaults: 'session-defaults.json',
      settings: 'settings.json',
      state: 'state.json',
    });
  });

  it('createAtomicWritePlan：目标存在 → 计划备份旧值；相同内容不重复备份', () => {
    const plan = createAtomicWritePlan('old', 'new');
    expect(plan.backupContent).toBe('old');
    expect(plan.backupPathSuffix).toBe('.bak');
    expect(plan.tmpPathSuffix).toMatch(/^\.tmp-/);
    expect(plan.finalContent).toBe('new');

    const noop = createAtomicWritePlan('same', 'same');
    expect(noop.backupContent).toBeNull();

    const fresh = createAtomicWritePlan(null, 'first');
    expect(fresh.backupContent).toBeNull();
  });

  it('recoverWithBackup：主文件正常 → primary', () => {
    const doc = emptySessionsDoc();
    const r = recoverWithBackup(serializeDoc(doc), 'garbage{', parseSessionsDoc);
    expect(r.source).toBe('primary');
    expect(r.doc?.sessions).toEqual([]);
  });

  it('recoverWithBackup：主损坏 + .bak 可用 → 自动回滚 backup（R12 演练）', () => {
    const doc = emptySessionsDoc();
    doc.sessions.push({
      id: 's-1',
      kind: 'folder-compare',
      name: '备份恢复',
      locked: false,
      createdAt: '2026-01-01T00:00:00.000Z',
      updatedAt: '2026-01-01T00:00:00.000Z',
      specs: { left: '/a', right: '/b' },
      overrides: {},
      state: {},
      folderId: null,
    });
    const r = recoverWithBackup(
      '{"version":1,"sessions":[破损',
      serializeDoc(doc),
      parseSessionsDoc,
    );
    expect(r.source).toBe('backup');
    expect(r.doc?.sessions[0].name).toBe('备份恢复');
    expect(r.primaryError).toBeTruthy();
  });

  it('recoverWithBackup：主与 .bak 均损坏 → lost（UI 提示自动恢复失败）', () => {
    const r = recoverWithBackup('{{{', 'not-json', parseSessionsDoc);
    expect(r.source).toBe('lost');
    expect(r.doc).toBeNull();
    expect(r.primaryError).toBeTruthy();
    expect(r.backupError).toBeTruthy();
  });

  it('recoverWithBackup：双文件皆缺失 → 全新安装（不算损坏）', () => {
    const r = recoverWithBackup(null, null, parseSessionsDoc);
    expect(r.source).toBe('primary');
    expect(r.doc).toBeNull();
    expect(r.primaryError).toBeUndefined();
  });

  it('recoverWithBackup：主缺失但 .bak 存在 → 从 .bak 恢复（写入中断场景）', () => {
    const doc = serializeDoc(emptySessionsDoc());
    const r = recoverWithBackup(null, doc, parseSessionsDoc);
    expect(r.source).toBe('backup');
    expect(r.doc).not.toBeNull();
    expect(r.primaryError).toContain('缺失');
  });

  it('recoverWithBackup：ParseError 与 JSON 异常都视为损坏', () => {
    const r = recoverWithBackup('{"version": 2}', null, parseSessionsDoc);
    expect(r.source).toBe('lost');
    expect(r.primaryError).toContain('版本');
  });

  it('pushMru：去重置前 + 截断上限', () => {
    let list = pushMru(['a', 'b', 'c'], 'b', 3);
    expect(list).toEqual(['b', 'a', 'c']);
    list = pushMru(list, 'd', 3);
    expect(list).toEqual(['d', 'b', 'a']);
    expect(pushMru([], 'x')).toEqual(['x']);
    expect(pushMru(['1', '2', '3'], '4', 2)).toEqual(['4', '1']);
  });

  it('normalizePathInput：去空白', () => {
    expect(normalizePathInput(' /tmp/L ')).toBe('/tmp/L');
    expect(normalizePathInput('')).toBe('');
  });

  it('损坏的 JSON 触发 ParseError 文案（供 UI 呈现）', () => {
    try {
      parseSessionsDoc('xx');
      expect.unreachable();
    } catch (err) {
      expect(err).toBeInstanceOf(ParseError);
      expect((err as ParseError).message).toContain('不是合法 JSON');
    }
  });
});
