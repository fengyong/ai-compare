import { describe, expect, it } from 'vitest';
import {
  BUILTIN_FORMAT_CATALOG,
  defaultFileFormatsState,
  effectiveFormats,
  exportFormatPack,
  firstMatchingFormat,
  importFormatPack,
  maskListText,
  maskMatches,
  newCustomFormat,
  parseMaskList,
  removeCustom,
  setBuiltinOverride,
  shadowedFormatIds,
  upsertCustom,
  validateFormatSpec,
} from './formatsModel';

describe('settings/formatsModel（P10 文件格式页纯逻辑）', () => {
  // ---- 掩码语义（FR-FLT-001，与引擎 mask_matches 同构） ----

  it('掩码：* 与 *.* 匹配任意文件名（含无扩展名）', () => {
    expect(maskMatches('*.*', 'Makefile')).toBe(true);
    expect(maskMatches('*.*', 'a.txt')).toBe(true);
    expect(maskMatches('*', 'x.tar.gz')).toBe(true);
  });

  it('掩码：末尾句点仅匹配无扩展名文件且通配符不吞句点', () => {
    expect(maskMatches('*.', 'Makefile')).toBe(true);
    expect(maskMatches('*.', 'a.txt')).toBe(false);
    expect(maskMatches('?.', 'a')).toBe(true); // 恰匹配单字符无扩展名
    expect(maskMatches('?.', 'ab')).toBe(false); // ? 只匹配一个字符
    expect(maskMatches('?.', 'a.txt')).toBe(false);
  });

  it('掩码：? 单字符、[az]/[a-z]/[!az] 字符类', () => {
    expect(maskMatches('?.c', 'a.c')).toBe(true);
    expect(maskMatches('?.c', 'ab.c')).toBe(false);
    expect(maskMatches('[abc].txt', 'b.txt')).toBe(true);
    expect(maskMatches('[abc].txt', 'd.txt')).toBe(false);
    expect(maskMatches('[a-z0-9].log', '7.log')).toBe(true);
    expect(maskMatches('[!abc].log', 'd.log')).toBe(true);
    expect(maskMatches('[!abc].log', 'a.log')).toBe(false);
  });

  it('掩码：大小写不敏感、空掩码不匹配、编辑框文本解析去重', () => {
    expect(maskMatches('*.C', 'foo.c')).toBe(true);
    expect(maskMatches('', 'anything')).toBe(false);
    expect(parseMaskList(' *.cpp; *.h ,,*.cpp ')).toEqual(['*.cpp', '*.h']);
    expect(maskListText(['*.cpp', '*.h'])).toBe('*.cpp; *.h');
  });

  // ---- 内置目录 / 生效列表 / 首匹配 ----

  it('内置目录 ≥12 语言且 id 唯一；内置 cpp 带注释元素', () => {
    expect(BUILTIN_FORMAT_CATALOG.length).toBeGreaterThanOrEqual(12);
    const ids = BUILTIN_FORMAT_CATALOG.map((f) => f.id);
    expect(new Set(ids).size).toBe(ids.length);
    const cpp = BUILTIN_FORMAT_CATALOG.find((f) => f.id === 'builtin-cpp')!;
    expect(cpp.grammar.map((e) => e.name)).toContain('注释');
    expect(cpp.masks).toContain('*.cpp');
  });

  it('首匹配生效：列表顺序优先，禁用项跳过，空掩码不参与', () => {
    const base = defaultFileFormatsState();
    const a = { ...newCustomFormat('fmt-a'), masks: ['*.log'] };
    const b = { ...newCustomFormat('fmt-b'), masks: ['*.log'] };
    const st1 = upsertCustom(upsertCustom(base, a), b);
    const all1 = effectiveFormats(st1);
    expect(firstMatchingFormat(all1, 'x.log')?.id).toBe('fmt-a') /* '先注册者生效' */;

    // 禁用 fmt-a 后 fmt-b 接管
    const st2 = setBuiltinOverride(st1, 'fmt-a', { enabled: false });
    // fmt-a 是 custom，禁用走 upsert
    const st2b = upsertCustom(st1, { ...a, enabled: false });
    const all2 = effectiveFormats(st2b);
    expect(firstMatchingFormat(all2, 'x.log')?.id).toBe('fmt-b');
    expect(firstMatchingFormat(effectiveFormats(st2), 'x.log')?.id).toBe('fmt-a') /* 'overrides 仅作用于内置 id' */;
    void st2;

    // 空掩码格式不匹配
    const noMask = newCustomFormat('fmt-empty');
    const all3 = effectiveFormats(upsertCustom(base, noMask));
    expect(firstMatchingFormat(all3, 'anything.txt')).toBeNull();
  });

  it('遮蔽检测：更高项的相同掩码或全匹配使后者红显', () => {
    const base = defaultFileFormatsState();
    const catchAll = { ...newCustomFormat('fmt-all'), masks: ['*'] };
    const specific = { ...newCustomFormat('fmt-spec'), masks: ['*.log'] };
    let st = upsertCustom(upsertCustom(base, catchAll), specific);
    let shadowed = shadowedFormatIds(effectiveFormats(st));
    expect(shadowed.has('fmt-spec')).toBe(true) /* '被 * 全匹配遮蔽' */;
    expect(shadowed.has('fmt-all')).toBe(false);

    // 相同掩码互相遮蔽（先者生效）
    st = upsertCustom(upsertCustom(base, { ...newCustomFormat('m1'), masks: ['*.log'] }), {
      ...newCustomFormat('m2'),
      masks: ['*.log'],
    });
    shadowed = shadowedFormatIds(effectiveFormats(st));
    expect(shadowed.has('m2')).toBe(true);
    expect(shadowed.has('m1')).toBe(false);

    // 禁用项不遮蔽他人
    st = upsertCustom(upsertCustom(base, { ...newCustomFormat('off'), masks: ['*.log'], enabled: false }), {
      ...newCustomFormat('on'),
      masks: ['*.log'],
    });
    shadowed = shadowedFormatIds(effectiveFormats(st));
    expect(shadowed.has('on')).toBe(false);
  });

  it('内置覆盖：enabled/masks 可覆盖且不影响内置目录原表', () => {
    // 掩码收窄（保持启用）：*.c 不再命中，*.cpp 仍命中
    const st = setBuiltinOverride(defaultFileFormatsState(), 'builtin-cpp', { masks: ['*.cpp'] });
    const cpp = effectiveFormats(st).find((f) => f.id === 'builtin-cpp')!;
    expect(cpp.masks).toEqual(['*.cpp']);
    expect(cpp.enabled).toBe(true);
    expect(BUILTIN_FORMAT_CATALOG.find((f) => f.id === 'builtin-cpp')!.masks).toContain('*.c'); // 原表不可变
    expect(firstMatchingFormat(effectiveFormats(st), 'main.c')).toBeNull();
    expect(firstMatchingFormat(effectiveFormats(st), 'a.cpp')?.id).toBe('builtin-cpp');

    // 禁用后不再命中
    const st2 = setBuiltinOverride(defaultFileFormatsState(), 'builtin-cpp', { enabled: false });
    expect(firstMatchingFormat(effectiveFormats(st2), 'a.cpp')).toBeNull();
  });

  // ---- 导入导出 ----

  it('格式集导出->导入 roundtrip；版本守卫与坏 JSON 拒绝', () => {
    let st = defaultFileFormatsState();
    st = upsertCustom(st, { ...newCustomFormat('team-x'), masks: ['*.x'], description: '团队格式' });
    st = setBuiltinOverride(st, 'builtin-json', { enabled: false });

    const json = exportFormatPack(st);
    const parsed = JSON.parse(json);
    expect(parsed.version).toBe(1);
    expect(parsed.kind).toBe('ai-compare-format-pack');
    expect(parsed.formats.length).toBe(BUILTIN_FORMAT_CATALOG.length + 1);

    const back = importFormatPack(defaultFileFormatsState(), json);
    expect(back.ok).toBe(true);
    if (back.ok) {
      expect(back.loaded).toBe(parsed.formats.length) /* '全量导入' */;
      const x = back.state.custom.find((f) => f.id === 'team-x');
      expect(x?.masks).toEqual(['*.x']);
      // 内置导入落 overrides
      expect(back.state.builtinOverrides['builtin-json']?.enabled).toBe(false);
    }

    const future = importFormatPack(defaultFileFormatsState(), '{"version":2,"formats":[]}');
    expect(future.ok).toBe(false);
    const bad = importFormatPack(defaultFileFormatsState(), 'not json');
    expect(bad.ok).toBe(false);
    const noFormats = importFormatPack(defaultFileFormatsState(), '{"version":1}');
    expect(noFormats.ok).toBe(false);
  });

  it('删除自定义格式；新增默认格式带 // 注释元素', () => {
    let st = upsertCustom(defaultFileFormatsState(), newCustomFormat('fmt-del'));
    expect(effectiveFormats(st).some((f) => f.id === 'fmt-del')).toBe(true);
    st = removeCustom(st, 'fmt-del');
    expect(effectiveFormats(st).some((f) => f.id === 'fmt-del')).toBe(false);

    const fresh = newCustomFormat('fmt-new');
    expect(validateFormatSpec({ ...fresh, masks: ['*.new'] })).toEqual([]);
    expect(fresh.grammar[0].items[0]).toMatchObject({ kind: 'delimited', start: '//', toEol: true });
    expect(validateFormatSpec(fresh)).toHaveLength(1) /* '空掩码报错' */;
    expect(validateFormatSpec({ ...fresh, masks: ['*.x'], id: 'builtin-fake' })).toHaveLength(1) /* '内置前缀保留' */;
  });
});
