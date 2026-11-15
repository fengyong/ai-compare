import { describe, expect, it } from 'vitest';
import type { DirRow } from '../../protocol/types';
import {
  applyDosMask,
  applyRegexTemplate,
  compileRegex,
  firstIllegalChar,
  planRenames,
  renamesToOps,
  splitBaseExt,
  type RegexRenameOptions,
} from './renameTemplate';

/** FR-DIR-064 批量重命名模板引擎（DOS 掩码 / 正则模板 / 冲突检测） */
describe('renameTemplate', () => {
  // ---------------- DOS 掩码 ----------------

  it('DOS：`*` 保留原名基底（去扩展名）', () => {
    expect(applyDosMask('abc.txt', { mask: '*.bak', counterStart: 1 }, 1)).toBe('abc.bak');
  });

  it('DOS：`IMG####.jpg` 计数器零填充（4 位）', () => {
    expect(applyDosMask('a.png', { mask: 'IMG####.jpg', counterStart: 1 }, 1)).toBe('IMG0001.jpg');
    expect(applyDosMask('b.png', { mask: 'IMG####.jpg', counterStart: 1 }, 42)).toBe('IMG0042.jpg');
  });

  it('DOS：`#` 宽度按串长（1 位/3 位）', () => {
    expect(applyDosMask('x.bin', { mask: 'n#', counterStart: 1 }, 7)).toBe('n7');
    expect(applyDosMask('x.bin', { mask: 'n###', counterStart: 1 }, 7)).toBe('n007');
  });

  it('DOS：计数器起始值可配置', () => {
    expect(applyDosMask('a', { mask: 'IMG###', counterStart: 100 }, 100)).toBe('IMG100');
  });

  it('DOS：`?` 依序消费原基底字符（abc?.bak 保留原字符生成唯一名）', () => {
    expect(applyDosMask('abcd.txt', { mask: 'abc?.bak', counterStart: 1 }, 1)).toBe('abcd.bak');
    expect(applyDosMask('x9y.txt', { mask: '??z', counterStart: 1 }, 1)).toBe('x9z');
  });

  it('DOS：`?` 超出原基底长度时产出空', () => {
    expect(applyDosMask('ab.txt', { mask: '?????', counterStart: 1 }, 1)).toBe('ab');
  });

  it('DOS：首个 `#` 串为计数器，后续 `#` 串按字面量', () => {
    expect(applyDosMask('a', { mask: 'IMG####_####', counterStart: 1 }, 3)).toBe('IMG0003_####');
  });

  it('DOS：点文件视为无扩展名基底', () => {
    expect(splitBaseExt('.gitignore')).toEqual({ base: '.gitignore', ext: '' });
    expect(applyDosMask('.gitignore', { mask: '*.bak', counterStart: 1 }, 1)).toBe(
      '.gitignore.bak',
    );
  });

  it('DOS：掩码无通配符 = 整名替换（同名冲突由计划层拦截）', () => {
    expect(applyDosMask('a.txt', { mask: 'fixed.log', counterStart: 1 }, 1)).toBe('fixed.log');
  });

  // ---------------- 正则模板 ----------------

  it('正则：数字捕获组替换为 xyz$1', () => {
    const opts: RegexRenameOptions = { pattern: '.*(\\d\\.txt)', template: 'xyz$1' };
    const compiled = compileRegex(opts);
    expect(compiled).toHaveProperty('re');
    expect(applyRegexTemplate('file1.txt', opts, compiled as { re: RegExp })).toBe('xyz1.txt');
  });

  it('正则：未命中行返回 null（保留原名，非错误）', () => {
    const opts: RegexRenameOptions = { pattern: '^(\\d+)-', template: 'v$1_' };
    const compiled = compileRegex(opts);
    expect(applyRegexTemplate('report.txt', opts, compiled as { re: RegExp })).toBeNull();
    expect(applyRegexTemplate('12-ab.txt', opts, compiled as { re: RegExp })).toBe('v12_ab.txt');
  });

  it('正则：非法正则返回编译错误', () => {
    const r = compileRegex({ pattern: '([unclosed', template: 'x' });
    expect(r).toHaveProperty('error');
  });

  it('正则：模板引用越界组报错（掩码 1 组引用 $2）', () => {
    const r = compileRegex({ pattern: '(a)', template: '$2' });
    expect(r).toHaveProperty('error');
    expect((r as { error: string }).error).toContain('$2');
  });

  it('正则：空旧掩码报错', () => {
    expect(compileRegex({ pattern: '', template: 'x' })).toHaveProperty('error');
  });

  // ---------------- 非法字符 ----------------

  it('非法字符检测：路径分隔符与 Windows 保留集', () => {
    expect(firstIllegalChar('a/b')).toBe('/');
    expect(firstIllegalChar('a\\b')).toBe('\\');
    expect(firstIllegalChar('a:b')).toBe(':');
    expect(firstIllegalChar('a<b>|?*"')).not.toBeNull();
    expect(firstIllegalChar('a-b_c.txt')).toBeNull();
  });

  // ---------------- 批量计划与冲突拦截 ----------------

  function row(id: string, name = id.split('/').pop() ?? id): DirRow {
    const side = { size: 1, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 };
    return {
      id,
      name,
      kind: 'file',
      left: side,
      right: null,
      status: 'leftOnly',
      hasChildren: false,
      hidden: false,
      suppressed: false,
      leftPath: id,
    };
  }

  const dos = { mask: 'IMG####.jpg', counterStart: 1 };
  const regex: RegexRenameOptions = { pattern: '^(old_)(.+)$', template: 'new_$2' };

  it('计划：DOS 批量按行序分配计数器，可执行清单完整', () => {
    const plan = planRenames({
      rows: [row('a.png'), row('b.png')],
      allRows: [row('a.png'), row('b.png')],
      mode: 'dos',
      dos,
      regex,
    });
    expect(plan.items.map((i) => i.newName)).toEqual(['IMG0001.jpg', 'IMG0002.jpg']);
    expect(plan.renames).toHaveLength(2);
    expect(plan.conflicts).toBe(0);
  });

  it('计划：批次内重名拦截（同掩码同名输出）', () => {
    const plan = planRenames({
      rows: [row('a.png'), row('b.png')],
      allRows: [row('a.png'), row('b.png')],
      mode: 'dos',
      dos: { mask: 'same.jpg', counterStart: 1 },
      regex,
    });
    expect(plan.conflicts).toBe(2);
    expect(plan.renames).toHaveLength(0);
    expect(plan.items.every((i) => i.conflict === 'duplicate')).toBe(true);
  });

  it('计划：批次内重名按大小写不敏感判定', () => {
    const plan = planRenames({
      rows: [row('a.png'), row('b.png')],
      allRows: [row('a.png'), row('b.png')],
      mode: 'dos',
      dos: { mask: 'Same.jpg', counterStart: 1 },
      regex,
    });
    expect(plan.conflicts).toBe(2);
  });

  it('计划：与未参与重命名的现有行撞名拦截（exists）', () => {
    const rows = [row('a.png'), row('b.png'), row('IMG0001.jpg')];
    const plan = planRenames({ rows: [rows[0], rows[1]], allRows: rows, mode: 'dos', dos, regex });
    expect(plan.items[0].conflict).toBe('exists');
    expect(plan.items[0].conflictReason).toContain('已存在');
    expect(plan.items[1].conflict).toBeNull();
    expect(plan.conflicts).toBe(1);
    expect(plan.renames).toHaveLength(1);
  });

  it('计划：参与重命名的行自身不构成背景撞名', () => {
    // a.png -> IMG0001.jpg 且 b.png -> IMG0002.jpg：IMG0002.jpg 不存在，均放行
    const rows = [row('a.png'), row('b.png')];
    const plan = planRenames({ rows, allRows: rows, mode: 'dos', dos, regex });
    expect(plan.conflicts).toBe(0);
  });

  it('计划：结果与原名相同 = unchanged（不入执行清单）', () => {
    const rows = [row('IMG0001.jpg')];
    const plan = planRenames({ rows, allRows: rows, mode: 'dos', dos, regex });
    expect(plan.items[0].state).toBe('unchanged');
    expect(plan.renames).toHaveLength(0);
    expect(plan.unchanged).toBe(1);
  });

  it('计划：正则模式未命中行保留原名（unchanged）', () => {
    const rows = [row('old_a.txt'), row('keep.txt')];
    const plan = planRenames({ rows, allRows: rows, mode: 'regex', dos, regex });
    expect(plan.items[0].state).toBe('rename');
    expect(plan.items[0].newName).toBe('new_a.txt');
    expect(plan.items[1].state).toBe('unchanged');
    expect(plan.conflicts).toBe(0);
  });

  it('计划：正则整掩码非法 -> 全部 invalid', () => {
    const plan = planRenames({
      rows: [row('old_a.txt')],
      allRows: [row('old_a.txt')],
      mode: 'regex',
      dos,
      regex: { pattern: '([bad', template: 'x' },
    });
    expect(plan.items[0].state).toBe('invalid');
    expect(plan.conflicts).toBe(1);
  });

  it('计划：正则模板注入路径分隔符 -> 非法字符拦截', () => {
    const plan = planRenames({
      rows: [row('old_a.txt')],
      allRows: [row('old_a.txt')],
      mode: 'regex',
      dos,
      regex: { pattern: '^old_(.*)$', template: 'sub/$1' },
    });
    expect(plan.items[0].state).toBe('invalid');
    expect(plan.items[0].reason).toContain('非法字符');
  });

  it('计划：目录行同机制重命名；执行序子先父后', () => {
    const dir = { ...row('old_sub'), kind: 'dir' as const, hasChildren: true };
    const child = row('old_sub/old_c.txt');
    const plan = planRenames({
      rows: [dir, child],
      allRows: [dir, child],
      mode: 'regex',
      dos,
      regex: { pattern: '^old_', template: 'new_' },
    });
    expect(plan.renames.map((r) => r.row.id)).toEqual(['old_sub/old_c.txt', 'old_sub']);
    expect(plan.renames[1].newName).toBe('new_sub');
  });

  it('计划：目录重命名与同父现有行撞名（exists）', () => {
    const dir = { ...row('sub'), kind: 'dir' as const, hasChildren: true };
    const other = row('sub2');
    const plan = planRenames({
      rows: [dir],
      allRows: [dir, other],
      mode: 'dos',
      dos: { mask: 'sub2', counterStart: 1 },
      regex,
    });
    expect(plan.items[0].conflict).toBe('exists');
  });

  it('计划：计数器起始偏移参与冲突判定（起始 1 已存在时改起始可解）', () => {
    const rows = [row('a.png'), row('b.png')];
    const all = [...rows, row('IMG0001.jpg'), row('IMG0002.jpg')];
    expect(planRenames({ rows, allRows: all, mode: 'dos', dos, regex }).conflicts).toBe(2);
    const plan2 = planRenames({
      rows,
      allRows: all,
      mode: 'dos',
      dos: { mask: 'IMG####.jpg', counterStart: 3 },
      regex,
    });
    expect(plan2.conflicts).toBe(0);
    expect(plan2.items.map((i) => i.newName)).toEqual(['IMG0003.jpg', 'IMG0004.jpg']);
  });

  it('renamesToOps：side 取存在侧（左优先）且载荷同形 DirExecuteOp', () => {
    const r1 = row('old_a.txt');
    const r2 = {
      ...row('old_b.txt'),
      left: null,
      leftPath: undefined,
      right: { size: 1, mtimeMs: 1, attribute: null, unixMode: 0, uid: 0, gid: 0 },
      rightPath: 'old_b.txt',
    };
    const plan = planRenames({ rows: [r1, r2], allRows: [r1, r2], mode: 'regex', dos, regex });
    const ops = renamesToOps(plan.renames.map((r) => ({ row: r.row, newName: r.newName! })));
    expect(ops).toEqual([
      { kind: 'rename', path: 'old_a.txt', side: 'left', newName: 'new_a.txt' },
      { kind: 'rename', path: 'old_b.txt', side: 'right', newName: 'new_b.txt' },
    ]);
  });
});
