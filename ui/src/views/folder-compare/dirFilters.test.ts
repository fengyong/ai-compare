import { describe, expect, it } from 'vitest';
import type { DirRow } from '../../protocol/types';
import {
  applyDisplayToRows,
  displayFilterPreset,
  displayVisible,
  fileAllowedBySpec,
  maskInputFromSpec,
  parseMaskInput,
  SHOW_ALL_DISPLAY_FILTER,
  wildcardMatch,
} from './dirFilters';

function row(partial: Partial<DirRow> & { id: string }): DirRow {
  const size = 10;
  const mtime = 1000;
  const left = partial.left !== undefined ? partial.left : { size, mtimeMs: mtime, attribute: null, unixMode: 0, uid: 0, gid: 0 };
  const right = partial.right !== undefined ? partial.right : { size, mtimeMs: mtime, attribute: null, unixMode: 0, uid: 0, gid: 0 };
  return {
    id: partial.id,
    name: partial.name ?? partial.id,
    kind: partial.kind ?? 'file',
    left,
    right,
    status: partial.status ?? 'same',
    hasChildren: partial.hasChildren ?? false,
    hidden: false,
    suppressed: false,
    leftPath: left ? partial.id : undefined,
    rightPath: right ? partial.id : undefined,
  };
}

describe('dirFilters：掩码匹配（BC 通配符子集）', () => {
  it('? 单字符与 * 任意字符', () => {
    expect(wildcardMatch('*.txt', 'a.txt')).toBe(true);
    expect(wildcardMatch('*.txt', 'a.log')).toBe(false);
    expect(wildcardMatch('a?c', 'abc')).toBe(true);
    expect(wildcardMatch('a?c', 'ac')).toBe(false);
    expect(wildcardMatch('*', 'anything.txt')).toBe(true);
  });

  it('字符集合 [seq] / [a-z] / [!seq]', () => {
    expect(wildcardMatch('[abc]x', 'ax')).toBe(true);
    expect(wildcardMatch('[abc]x', 'dx')).toBe(false);
    expect(wildcardMatch('x[0-9]', 'x5')).toBe(true);
    expect(wildcardMatch('x[!0-9]', 'x5')).toBe(false);
    expect(wildcardMatch('x[!0-9]', 'xa')).toBe(true);
  });

  it('大小写不敏感默认（BC 名称过滤器惯例）', () => {
    expect(wildcardMatch('*.TXT', 'file.txt')).toBe(true);
    expect(wildcardMatch('*.TXT', 'file.txt', false)).toBe(false);
  });
});

describe('dirFilters：名称过滤器输入解析（FR-DIR-031）', () => {
  it('分号分隔 + 排除前缀 + 目录尾反斜杠', () => {
    const spec = parseMaskInput('*.txt; -*.log; -build\\');
    expect(spec.includeFiles).toEqual(['*.txt']);
    expect(spec.excludeFiles).toEqual(['*.log']);
    expect(spec.excludeDirs).toEqual(['build']);
    expect(spec.includeDirs).toEqual([]);
    expect(spec.caseSensitive).toBeNull();
  });

  it('空输入 = 无过滤', () => {
    const spec = parseMaskInput('  ');
    expect(spec.includeFiles.length + spec.excludeFiles.length + spec.includeDirs.length + spec.excludeDirs.length).toBe(0);
  });

  it('输入↔回显往返', () => {
    const spec = parseMaskInput('*.cpp; -temp\\');
    const text = maskInputFromSpec(spec);
    const spec2 = parseMaskInput(text.replace(/; /g, ';'));
    expect(spec2.includeFiles).toEqual(spec.includeFiles);
    expect(spec2.excludeDirs).toEqual(spec.excludeDirs);
  });
});

describe('dirFilters：文件求值', () => {
  it('include 未命中 = 排除；exclude 命中 = 排除', () => {
    const spec = parseMaskInput('*.txt; -*secret*');
    expect(fileAllowedBySpec(['a.txt'], spec)).toBe(true);
    expect(fileAllowedBySpec(['a.md'], spec)).toBe(false);
    expect(fileAllowedBySpec(['my-secret-file.txt'], spec)).toBe(false);
  });

  it('目录排除掩码命中父目录 = 该文件排除', () => {
    const spec = parseMaskInput('-build\\');
    expect(fileAllowedBySpec(['src', 'x.rs'], spec)).toBe(true);
    expect(fileAllowedBySpec(['build', 'out.o'], spec)).toBe(false);
  });

  it('目录链中间段相对掩码（a/b 形态）', () => {
    const spec = parseMaskInput('src/*.rs');
    expect(fileAllowedBySpec(['src', 'main.rs'], spec)).toBe(true);
    expect(fileAllowedBySpec(['docs', 'main.rs'], spec)).toBe(false);
  });
});

describe('dirFilters：显示过滤器（FR-DIR-033，与引擎同口径）', () => {
  it('same/dirSame 由 showSame 控制', () => {
    expect(displayVisible(row({ id: 'x', status: 'same' }), SHOW_ALL_DISPLAY_FILTER)).toBe(true);
    expect(displayVisible(row({ id: 'x', status: 'same' }), displayFilterPreset('diffOnly'))).toBe(false);
  });

  it('QuickDiff 按 mtime 分左右较新', () => {
    const lNewer = row({ id: 'x', status: 'quickDiff', right: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 } });
    const rNewer = row({ id: 'x', status: 'quickDiff', left: { size: 1, mtimeMs: 0, attribute: null, unixMode: 0, uid: 0, gid: 0 } });
    expect(displayVisible(lNewer, displayFilterPreset('leftNewer'))).toBe(true);
    expect(displayVisible(lNewer, displayFilterPreset('rightNewer'))).toBe(false);
    expect(displayVisible(rNewer, displayFilterPreset('rightNewer'))).toBe(true);
  });

  it('孤立项由 showLeftOnly/showRightOnly 控制', () => {
    const lo = row({ id: 'x', status: 'leftOnly', right: null });
    expect(displayVisible(lo, displayFilterPreset('leftOnly'))).toBe(true);
    expect(displayVisible(lo, displayFilterPreset('rightOnly'))).toBe(false);
  });
});

describe('dirFilters：抑制标记重算（FR-DIR-035 青绿语义）', () => {
  it('非抑制：不可见 -> hidden；抑制：不可见 -> suppressed（互斥）', () => {
    const rows = [row({ id: 'a', status: 'same' }), row({ id: 'b', status: 'contentDiff' })];
    const diffOnly = displayFilterPreset('diffOnly');
    const hiddenMode = applyDisplayToRows(rows, diffOnly, false);
    expect(hiddenMode.find((r) => r.id === 'a')!.hidden).toBe(true);
    expect(hiddenMode.find((r) => r.id === 'a')!.suppressed).toBe(false);
    const suppressMode = applyDisplayToRows(rows, diffOnly, true);
    expect(suppressMode.find((r) => r.id === 'a')!.hidden).toBe(false);
    expect(suppressMode.find((r) => r.id === 'a')!.suppressed).toBe(true);
    // 差异行不受影响
    expect(suppressMode.find((r) => r.id === 'b')!.suppressed).toBe(false);
  });

  it('applyDisplayToRows 不修改输入行（不可变）', () => {
    const rows = [row({ id: 'a', status: 'same' })];
    applyDisplayToRows(rows, displayFilterPreset('diffOnly'), false);
    expect(rows[0].hidden).toBe(false);
  });
});
