import { describe, expect, it } from 'vitest';
import type { MergeChunkDto, MergeChunkOrigin } from '../../protocol/types';
import {
  MERGE3_FILTERS,
  MERGE3_TAKE_ACTIONS,
  chunkCls,
  conflictKindLabel,
  decisionLabel,
  filterChunks,
  filterLabel,
  formatMergeStats,
  originLabel,
  paneLines,
  rangeText,
  stateLabel,
} from './Merge3View';

/** 构造完整 MergeChunkDto（协议必填字段全覆盖，避免测试夹具与引擎形态漂移） */
function mk(origin: MergeChunkOrigin, extra: Partial<MergeChunkDto> = {}): MergeChunkDto {
  return {
    id: 0,
    origin,
    base: { start: 0, end: 1 },
    left: { start: 0, end: 1 },
    right: { start: 0, end: 1 },
    conflictKind: undefined,
    conflictRows: undefined,
    conflictUnimportant: undefined,
    baseLines: ['b'],
    leftLines: ['l'],
    rightLines: ['r'],
    decided: false,
    decision: null,
    autoResolved: false,
    unresolved: false,
    outputLines: ['o'],
    ...extra,
  };
}

const ALL_ORIGINS: MergeChunkOrigin[] = ['stable', 'leftOnly', 'rightOnly', 'bothChanged', 'conflict'];

describe('merge3 视图：块着色（FR-TMRG-002/005 颜色语义）', () => {
  it('五种来源各自着色，未更改=灰', () => {
    expect(ALL_ORIGINS.map((o) => chunkCls(mk(o)))).toEqual([
      'm3-stable',
      'm3-left',
      'm3-right',
      'm3-both',
      'm3-conflict',
    ]);
  });

  it('手动编辑输出优先着黄（FR-TMRG-005），不改变来源语义', () => {
    const manual = mk('conflict', { decided: true, decision: 'manual' });
    expect(chunkCls(manual)).toBe('m3-manual');
    expect(originLabel(manual.origin)).toContain('冲突');
  });

  it('来源/冲突类别/决策动作/过滤器标签全覆盖', () => {
    for (const o of ALL_ORIGINS) expect(originLabel(o).length).toBeGreaterThan(0);
    expect(conflictKindLabel('modifyModify')).toContain('两侧修改');
    expect(conflictKindLabel('deleteModify')).toContain('删除');
    expect(conflictKindLabel('addAdd')).toContain('新增');
    expect(conflictKindLabel(null)).toBe('');
    for (const a of MERGE3_TAKE_ACTIONS) expect(decisionLabel(a).length).toBeGreaterThan(0);
    expect(decisionLabel('manual')).toContain('手动');
    expect(MERGE3_FILTERS.map(filterLabel)).toHaveLength(5);
    expect(new Set(MERGE3_FILTERS.map(filterLabel)).size).toBe(5);
  });
});

describe('merge3 视图：显示过滤器（FR-TMRG-002 组合语义）', () => {
  const chunks = ALL_ORIGINS.map((o, i) => mk(o, { id: i }));

  it('显示全部 / 只显示更改 / 只显示冲突', () => {
    expect(filterChunks(chunks, 'all').map((c) => c.id)).toEqual([0, 1, 2, 3, 4]);
    expect(filterChunks(chunks, 'changes').map((c) => c.id)).toEqual([1, 2, 3, 4]);
    expect(filterChunks(chunks, 'conflicts').map((c) => c.id)).toEqual([4]);
  });

  it('只显示左侧更改 = 左侧更改 + 相同更改 + 冲突；右侧对称', () => {
    expect(filterChunks(chunks, 'leftChanges').map((c) => c.id)).toEqual([1, 3, 4]);
    expect(filterChunks(chunks, 'rightChanges').map((c) => c.id)).toEqual([2, 3, 4]);
  });
});

describe('merge3 视图：决策状态与行号/统计', () => {
  it('状态优先级：manual > decided > autoResolved > unresolved', () => {
    expect(stateLabel(mk('stable'))).toBe('');
    expect(stateLabel(mk('conflict', { unresolved: true }))).toContain('未决');
    expect(stateLabel(mk('conflict', { autoResolved: true }))).toContain('自动');
    expect(stateLabel(mk('conflict', { decided: true, decision: 'left' }))).toContain('已采用');
    expect(stateLabel(mk('conflict', { decided: true, decision: 'manual' }))).toContain('手动');
  });

  it('半开行区间 → 1 起行号；空区间显示 —', () => {
    expect(rangeText({ start: 0, end: 0 })).toBe('—');
    expect(rangeText({ start: 0, end: 1 })).toBe('1');
    expect(rangeText({ start: 4, end: 7 })).toBe('5-7');
  });

  it('真实引擎空区间省略行内容字段时按空数组兜底（防渲染崩溃）', () => {
    // 右侧插入块：引擎不下发 baseLines/leftLines（ts-rs 仍声明必填数组）
    const c = {
      ...mk('rightOnly', {
        base: { start: 5, end: 5 },
        left: { start: 5, end: 5 },
        right: { start: 5, end: 6 },
        rightLines: ['R3'],
      }),
      baseLines: undefined,
      leftLines: undefined,
    } as unknown as MergeChunkDto;
    expect(paneLines(c, 'base')).toEqual([]);
    expect(paneLines(c, 'left')).toEqual([]);
    expect(paneLines(c, 'right')).toEqual(['R3']);
    expect(rangeText(c.base)).toBe('—');
  });

  it('合并统计文本含四项计数（FR-TMRG-007）', () => {
    const text = formatMergeStats({ chunkCount: 10, conflictCount: 3, unresolved: 2, decided: 1 });
    expect(text).toContain('10');
    expect(text).toContain('3');
    expect(text).toContain('2');
    expect(text).toContain('1');
  });
});
