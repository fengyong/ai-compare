import { describe, expect, it } from 'vitest';
import {
  buildAlignedDocs,
  findDiffSectionAtLine,
  mapLineBySections,
} from './alignment';
import type { Section } from '../../protocol/types';

/**
 * 滚动映射逻辑单测（任务要求的"滚动映射逻辑"覆盖，R10 风险项的数据层）。
 */

const sec = (s: Partial<Section> & { id: number; kind: Section['kind'] }): Section => ({
  left: null,
  right: null,
  importance: 'important',
  inline: 'none',
  ...s,
});

/** 构造一个标准场景：
 *  left（10 行）          right（12 行）
 *  0-2   equal            0-2   equal
 *  3-5   change (3)       3-6   change (4)
 *  6-7   delete (2)       —
 *  —                      7-9   insert (3)
 *  8-9   equal            10-11 equal
 */
function sampleSections(): Section[] {
  return [
    sec({ id: 0, kind: 'equal', left: { start: 0, end: 3 }, right: { start: 0, end: 3 } }),
    sec({ id: 1, kind: 'change', left: { start: 3, end: 6 }, right: { start: 3, end: 7 } }),
    sec({ id: 2, kind: 'delete', left: { start: 6, end: 8 }, right: null }),
    sec({ id: 3, kind: 'insert', left: null, right: { start: 7, end: 10 } }),
    sec({ id: 4, kind: 'equal', left: { start: 8, end: 10 }, right: { start: 10, end: 12 } }),
  ];
}

describe('buildAlignedDocs', () => {
  const L = ['l0', 'l1', 'l2', 'l3', 'l4', 'l5', 'l6', 'l7', 'l8', 'l9'];
  const R = ['r0', 'r1', 'r2', 'r3', 'r4', 'r5', 'r6', 'r7', 'r8', 'r9', 'r10', 'r11'];

  it('两侧文档行数相等且逐行 1:1 对齐（填充行补齐）', () => {
    const { left, right } = buildAlignedDocs(sampleSections(), L, R);
    expect(left.lines.length).toBe(right.lines.length);
    expect(left.rows.length).toBe(right.rows.length);
    // 3 + max(3,4)=4 + 2 + 3 + 2 = 14
    expect(left.rows.length).toBe(14);
  });

  it('equal 段两侧内容一一对应', () => {
    const { left, right } = buildAlignedDocs(sampleSections(), L, R);
    expect(left.lines.slice(0, 3)).toEqual(['l0', 'l1', 'l2']);
    expect(right.lines.slice(0, 3)).toEqual(['r0', 'r1', 'r2']);
    expect(left.rows[0]).toMatchObject({ leftLine: 0, rightLine: 0, status: 'equal', sectionId: 0 });
    expect(right.rows[0]).toMatchObject({ leftLine: 0, rightLine: 0, status: 'equal', sectionId: 0 });
  });

  it('change 段：行数取 max，短侧缺行处为 absent（顶对齐）', () => {
    const { left, right } = buildAlignedDocs(sampleSections(), L, R);
    // change 段：rows 3..6（4 行），左侧行 3,4,5 + 1 个填充
    expect(left.lines[3]).toBe('l3');
    expect(left.lines[6]).toBe(''); // 填充
    expect(right.lines[6]).toBe('r6');
    expect(left.rows[6]).toMatchObject({ leftLine: undefined, rightLine: 6, status: 'absent', sectionId: 1 });
    expect(right.rows[5]).toMatchObject({ leftLine: 5, rightLine: 5, status: 'changed', sectionId: 1 });
  });

  it('delete 段：右侧行全为填充 absent', () => {
    const { left, right } = buildAlignedDocs(sampleSections(), L, R);
    // rows 7..8
    expect(left.lines[7]).toBe('l6');
    expect(right.lines[7]).toBe('');
    expect(left.rows[7]).toMatchObject({ leftLine: 6, rightLine: undefined, status: 'deleted', sectionId: 2 });
    expect(right.rows[7]).toMatchObject({ status: 'absent', sectionId: 2 });
  });

  it('insert 段：左侧行全为填充 absent', () => {
    const { left, right } = buildAlignedDocs(sampleSections(), L, R);
    // rows 9..11
    expect(right.lines[9]).toBe('r7');
    expect(left.lines[9]).toBe('');
    expect(left.rows[9]).toMatchObject({ status: 'absent', sectionId: 3 });
    expect(right.rows[9]).toMatchObject({ rightLine: 7, status: 'inserted', sectionId: 3 });
  });

  it('行号元数据在两侧一致（同一对齐行共享 leftLine/rightLine）', () => {
    const { left, right } = buildAlignedDocs(sampleSections(), L, R);
    for (let i = 0; i < left.rows.length; i++) {
      expect(right.rows[i].leftLine).toBe(left.rows[i].leftLine);
      expect(right.rows[i].rightLine).toBe(left.rows[i].rightLine);
    }
  });
});

describe('mapLineBySections', () => {
  const S = sampleSections();

  it('空 sections：恒等映射并 clamp', () => {
    expect(mapLineBySections([], 'left', 5, 10)).toBe(5);
    expect(mapLineBySections([], 'left', 99, 10)).toBe(9);
    expect(mapLineBySections([], 'right', 3, 0)).toBeNull();
  });

  it('equal 段内偏移一一对应', () => {
    expect(mapLineBySections(S, 'left', 1, 12)).toBe(1);
    expect(mapLineBySections(S, 'right', 11, 10)).toBe(9); // 尾部 equal：r10→l8, r11→l9
  });

  it('change 段：顶对齐，偏移越界取对侧末行', () => {
    // l3 → r3, l4 → r4, l5 → r5（change 左3行/右4行）
    expect(mapLineBySections(S, 'left', 3, 12)).toBe(3);
    expect(mapLineBySections(S, 'left', 4, 12)).toBe(4);
    // 反向：r6（右4行的第4行）→ 对侧只有3行 → clamp 到 l5
    expect(mapLineBySections(S, 'right', 6, 10)).toBe(5);
  });

  it('delete 段（左独有）：右侧行落在左侧行号上（l6→r7 间隙锚点）', () => {
    // delete 段之前的 otherPos = 7（右游标推进到 change 段末尾）
    // 左侧行 6 在 delete 段内、对侧无行 → 返回 otherPos = 7
    expect(mapLineBySections(S, 'left', 6, 12)).toBe(7);
    expect(mapLineBySections(S, 'left', 7, 12)).toBe(7);
  });

  it('insert 段精确语义核对', () => {
    // 左侧行 7（delete 段）之后、左侧行 8 属尾部 equal（left 8..10 ↔ right 10..12）
    // 左侧 walking：sec0 推进 (my=3,other=3)；sec1 (my=6,other=7)；sec2 delete (my=8,other=7)
    // sec3 insert：mRange=null 跳过；sec4 equal：line 8 < start 8? 否——line==start → 在段内
    //   equal → r10
    expect(mapLineBySections(S, 'left', 8, 12)).toBe(10);
    // 右侧行 8（insert 段内，7..10）→ 对侧无行 → otherPos = 8（delete 段后左游标）
    expect(mapLineBySections(S, 'right', 8, 10)).toBe(8);
    // 右侧行 9 → 同样 8
    expect(mapLineBySections(S, 'right', 9, 10)).toBe(8);
  });

  it('尾部间隙线性外推', () => {
    // 右侧行 11（尾部 equal 内 r11→l9）
    expect(mapLineBySections(S, 'right', 11, 10)).toBe(9);
    // 越界 clamp
    expect(mapLineBySections(S, 'right', 999, 10)).toBe(9);
    expect(mapLineBySections(S, 'left', 999, 12)).toBe(11);
  });

  it('负数行号 clamp 到 0', () => {
    expect(mapLineBySections(S, 'left', -3, 12)).toBe(0);
  });
});

describe('findDiffSectionAtLine', () => {
  const S = sampleSections();

  it('命中差异段', () => {
    expect(findDiffSectionAtLine(S, 'left', 4)?.id).toBe(1);
    expect(findDiffSectionAtLine(S, 'left', 6)?.id).toBe(2);
    expect(findDiffSectionAtLine(S, 'right', 8)?.id).toBe(3);
  });

  it('equal 行与越界返回 null', () => {
    expect(findDiffSectionAtLine(S, 'left', 1)).toBeNull();
    expect(findDiffSectionAtLine(S, 'left', 100)).toBeNull();
  });

  it('区间端点：start 命中、end 不命中', () => {
    expect(findDiffSectionAtLine(S, 'left', 3)?.id).toBe(1);
    expect(findDiffSectionAtLine(S, 'left', 5)?.id).toBe(1);
    // left 5 是 change 段末行（end=6 半开区间不含 5? end 6 → [3,6) 含 5）
    expect(findDiffSectionAtLine(S, 'left', 6)?.id).toBe(2);
  });
});
