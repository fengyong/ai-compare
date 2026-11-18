import { beforeEach, describe, expect, it } from 'vitest';
import {
  deleteTextSession,
  findDirtyTextSessionByPaths,
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