import { describe, expect, it } from 'vitest';
import type { DirContentCompareRow, DirRow, EngineCapabilities } from '../../protocol/types';
import { F } from './localStrings';
import {
  buildCompareContentParams,
  compareTargetIds,
  contentCompareSupported,
  isContentComparable,
  summarizeContentCompare,
} from './contentCompare';

const row = (id: string, over: Partial<DirRow> = {}): DirRow => ({
  id,
  name: id,
  kind: 'file',
  left: { size: 1, mtimeMs: 0, unixMode: 0, uid: 0, gid: 0 },
  right: { size: 1, mtimeMs: 0, unixMode: 0, uid: 0, gid: 0 },
  status: 'same',
  hasChildren: false,
  hidden: false,
  suppressed: false,
  ...over,
});

const caps = (v: boolean | undefined): EngineCapabilities =>
  ({ dirCompareContent: v }) as EngineCapabilities;

const result = (id: string, r: DirContentCompareRow['result']): DirContentCompareRow => ({
  id,
  result: r,
  status:
    r === 'binarySame' || r === 'rulesSame'
      ? 'same'
      : r === 'unimportantDiff'
        ? 'similar'
        : 'contentDiff',
});

describe('contentCompareSupported（能力位协商）', () => {
  it('旧引擎/未协商 → false，不得当成已比较', () => {
    expect(contentCompareSupported(null)).toBe(false);
    expect(contentCompareSupported(undefined)).toBe(false);
    expect(contentCompareSupported(caps(undefined))).toBe(false);
    expect(contentCompareSupported(caps(false))).toBe(false);
  });

  it('新引擎显式 true → 支持（新能力位默认 false，R3-22）', () => {
    expect(contentCompareSupported(caps(true))).toBe(true);
  });
});

describe('isContentComparable / compareTargetIds', () => {
  it('只有两侧都存在的文件行可比较（目录行与孤立项跳过）', () => {
    expect(isContentComparable(row('a.txt'))).toBe(true);
    expect(isContentComparable(row('d', { kind: 'dir' }))).toBe(false);
    expect(isContentComparable(row('l.txt', { right: undefined }))).toBe(false);
    expect(isContentComparable(row('r.txt', { left: undefined }))).toBe(false);
  });

  it('按选择集过滤，未选中的行不参与', () => {
    const rows = [
      row('a.txt'),
      row('b.txt'),
      row('d', { kind: 'dir' }),
      row('l.txt', { right: undefined }),
    ];
    expect(compareTargetIds(rows, new Set(['a.txt', 'b.txt', 'd', 'l.txt']))).toEqual([
      'a.txt',
      'b.txt',
    ]);
  });

  it('空选择 → 空数组 = 交引擎比较全部已配对文件（FR-DIR-012）', () => {
    expect(compareTargetIds([row('a.txt')], new Set())).toEqual([]);
  });
});

describe('buildCompareContentParams', () => {
  it('未指定方法 → 不传该键（R3-21：引擎按会话判据取默认）', () => {
    expect(buildCompareContentParams('s1', ['a.txt'])).toEqual({
      sessionId: 's1',
      ids: ['a.txt'],
      method: undefined,
    });
  });

  it('显式方法透传，且 ids 复制不共享调用方数组', () => {
    const ids = ['a.txt'];
    const p = buildCompareContentParams('s1', ids, 'rulesBased');
    expect(p.method).toBe('rulesBased');
    ids.push('b.txt');
    expect(p.ids).toEqual(['a.txt']);
  });
});

describe('summarizeContentCompare（五态结果）', () => {
  it('空结果 → 无可比较项文案', () => {
    const s = summarizeContentCompare([]);
    expect(s.total).toBe(0);
    expect(s.message).toBe(F.contentCompareEmpty);
  });

  it('五态各自计数，摘要只列非零态', () => {
    const s = summarizeContentCompare([
      result('a', 'binarySame'),
      result('b', 'binaryDiff'),
      result('c', 'rulesSame'),
      result('d', 'unimportantDiff'),
      result('e', 'importantDiff'),
      result('f', 'importantDiff'),
    ]);
    expect(s.total).toBe(6);
    expect(s.counts).toEqual({
      binarySame: 1,
      binaryDiff: 1,
      rulesSame: 1,
      unimportantDiff: 1,
      importantDiff: 2,
    });
    expect(s.message).toContain(`${F.contentCompareImportantDiff} 2`);
    expect(s.message).not.toContain('0');
  });

  it('CRC/二进制只产出前两态（不臆造规则结论）', () => {
    const s = summarizeContentCompare([result('a', 'binarySame'), result('b', 'binaryDiff')]);
    expect(s.counts.rulesSame).toBe(0);
    expect(s.counts.unimportantDiff).toBe(0);
    expect(s.counts.importantDiff).toBe(0);
    expect(s.message).toBe(`${F.contentCompareBinarySame} 1，${F.contentCompareBinaryDiff} 1`);
  });
});
