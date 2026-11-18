import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  deleteTextSession,
  findDirtyTextSessionByPaths,
  getTextSession,
  isTextModified,
  newTextSessionModel,
  putTextSession,
} from './textModel';

const L = '/tmp/a.txt';
const R = '/tmp/b.txt';
const ZERO_SUMMARY = {
  sectionCount: 0,
  equalSections: 0,
  insertSections: 0,
  deleteSections: 0,
  changeSections: 0,
  equalLines: 0,
  insertedLines: 0,
  deletedLines: 0,
};

function seed(sessionId: string, dirty: boolean, leftPath = L, rightPath = R): void {
  const model = newTextSessionModel({
    sessionId,
    leftPath,
    rightPath,
    leftName: 'a.txt',
    rightName: 'b.txt',
    leftLines: ['x'],
    rightLines: ['y'],
    sections: [],
    summary: { ...ZERO_SUMMARY },
  });
  model.modifiedSides = { left: dirty, right: false };
  putTextSession(model);
}

beforeEach(() => {
  for (const id of ['s1', 's2', 's3']) deleteTextSession(id);
});

describe('findDirtyTextSessionByPaths', () => {
  it('只认领已死且脏的会话；活会话/干净模型/路径不符都不认', () => {
    seed('s1', true);
    const dead = (id: string) => id !== 's1';
    expect(findDirtyTextSessionByPaths(L, R, undefined, dead)?.sessionId).toBe('s1');
    // 活会话不认
    expect(findDirtyTextSessionByPaths(L, R, undefined, () => true)).toBeUndefined();
    // 排除自身
    expect(findDirtyTextSessionByPaths(L, R, 's1', dead)).toBeUndefined();
    // 路径不符
    expect(findDirtyTextSessionByPaths(L, '/other', undefined, dead)).toBeUndefined();
    // 干净模型不认
    deleteTextSession('s1');
    seed('s2', false);
    expect(findDirtyTextSessionByPaths(L, R, undefined, dead)).toBeUndefined();
  });

  it('isLive 缺省恒真 = 不传判定就不认领（安全缺省）', () => {
    seed('s3', true);
    expect(findDirtyTextSessionByPaths(L, R)).toBeUndefined();
  });
});

describe('putTextSession LRU 脏保护（UI-03）', () => {
  const ids: string[] = [];
  afterEach(() => {
    for (const id of ids.splice(0)) deleteTextSession(id);
  });

  function seedMany(n: number, dirty: boolean, prefix: string): void {
    for (let i = 0; i < n; i++) {
      const id = `${prefix}${i}`;
      ids.push(id);
      seed(id, dirty, `/lru/${prefix}l${i}.txt`, `/lru/${prefix}r${i}.txt`);
    }
  }

  it('25 个脏标签全部往返后仍在缓存且仍脏（淘汰跳过脏项）', () => {
    seedMany(25, true, 'd');
    // 往返：反序重新 put（模拟标签切换水合）
    for (let i = 24; i >= 0; i--) {
      seed(`d${i}`, true, `/lru/dl${i}.txt`, `/lru/dr${i}.txt`);
    }
    for (let i = 0; i < 25; i++) {
      const m = getTextSession(`d${i}`);
      expect(m, `d${i} 不得被淘汰`).toBeDefined();
      expect(isTextModified(m!)).toBe(true);
    }
  });

  it('脏净混合淘汰最旧干净项；全干净时 LRU 语义不变', () => {
    seedMany(19, true, 'm');
    seedMany(3, false, 'c');
    // 19 脏 + 3 净 = 22 > 20 → 淘汰两个最旧干净项
    for (let i = 0; i < 19; i++) expect(getTextSession(`m${i}`)).toBeDefined();
    expect(getTextSession('c0')).toBeUndefined();
    expect(getTextSession('c1')).toBeUndefined();
    expect(getTextSession('c2')).toBeDefined();
    for (const id of ids.splice(0)) deleteTextSession(id);
    seedMany(25, false, 'p');
    expect(getTextSession('p0')).toBeUndefined();
    expect(getTextSession('p24')).toBeDefined();
  });
});