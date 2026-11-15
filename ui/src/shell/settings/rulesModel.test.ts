import { describe, expect, it } from 'vitest';
import {
  DEFAULT_COMPARE_RULES,
  defaultCompareRulesState,
  effectiveElementsAcrossLayers,
  effectiveUnimportantElements,
  elementsOfFormat,
  mergeRuleLayers,
  setBinding,
  toImportanceOptionsSpec,
  toRulesSpec,
  toggleElement,
} from './rulesModel';
import { defaultFileFormatsState } from './formatsModel';

describe('settings/rulesModel（P10 比较规则页纯逻辑）', () => {
  it('缺省值与引擎 ImportanceOptions::default 一致（行尾/编码不重要 = true）', () => {
    const d = defaultCompareRulesState();
    expect(d.leadingWsUnimportant).toBe(false);
    expect(d.embeddedWsUnimportant).toBe(false);
    expect(d.trailingWsUnimportant).toBe(false);
    expect(d.caseUnimportant).toBe(false);
    expect(d.eolStyleUnimportant).toBe(true);
    expect(d.encodingUnimportant).toBe(true);
    expect(d.formatBindings).toEqual({});
    expect(DEFAULT_COMPARE_RULES).toEqual(d);
  });

  it('开关映射 ImportanceOptionsSpec：字段一一对应', () => {
    const spec = toImportanceOptionsSpec({
      ...defaultCompareRulesState(),
      caseUnimportant: true,
      trailingWsUnimportant: true,
    });
    expect(spec).toEqual({
      leadingWsUnimportant: false,
      embeddedWsUnimportant: false,
      trailingWsUnimportant: true,
      caseUnimportant: true,
      eolStyleUnimportant: true,
      encodingUnimportant: true,
    });
  });

  it('toRulesSpec：绑定格式取其元素集；未绑定/无格式为空集（不启用语法折叠）', () => {
    const st = setBinding(defaultCompareRulesState(), 'builtin-cpp', ['注释']);
    const withFmt = toRulesSpec(st, 'builtin-cpp');
    expect(withFmt.unimportantElements).toEqual(['注释']);
    expect(withFmt.caseUnimportant).toBe(false);

    expect(toRulesSpec(st, 'builtin-json').unimportantElements).toEqual([]); /* '未绑定格式' */
    expect(toRulesSpec(st, null).unimportantElements).toEqual([]); /* '无格式' */
  });

  it('elementsOfFormat：内置格式列出元素名；未知格式为空', () => {
    const fs = defaultFileFormatsState();
    expect(elementsOfFormat(fs, 'builtin-cpp')).toContain('注释');
    expect(elementsOfFormat(fs, 'builtin-cpp')).toContain('字符串');
    expect(elementsOfFormat(fs, 'builtin-yaml')).toEqual(['注释']);
    expect(elementsOfFormat(fs, 'no-such')).toEqual([]);
    expect(elementsOfFormat(fs, null)).toEqual([]);
  });

  it('元素勾选 toggle：加勾/去勾按格式分别记忆（集合按名排序稳定）', () => {
    let st = defaultCompareRulesState();
    st = toggleElement(st, 'builtin-cpp', '注释');
    st = toggleElement(st, 'builtin-cpp', '字符串');
    expect([...effectiveUnimportantElements(st, 'builtin-cpp')].sort()).toEqual(
      ['注释', '字符串'].sort(),
    );
    // 另一格式互不影响
    expect(effectiveUnimportantElements(st, 'builtin-json')).toEqual([]);
    // 去勾
    st = toggleElement(st, 'builtin-cpp', '注释');
    expect(effectiveUnimportantElements(st, 'builtin-cpp')).toEqual(['字符串']);
  });

  it('层合并：部分层键浅覆盖（未涉及键保留缺省），bindings 对象整体覆盖', () => {
    const global: Partial<typeof DEFAULT_COMPARE_RULES> = {
      caseUnimportant: true,
      formatBindings: { 'builtin-cpp': ['注释'] },
    };
    const kind: Partial<typeof DEFAULT_COMPARE_RULES> = {
      trailingWsUnimportant: true,
      formatBindings: { 'builtin-json': ['注释'] },
    };
    const merged = mergeRuleLayers([global, kind]);
    expect(merged.caseUnimportant).toBe(true); /* '低层键保留' */
    expect(merged.trailingWsUnimportant).toBe(true); /* '高层键覆盖' */
    expect(merged.eolStyleUnimportant).toBe(true); /* '两层都未涉及 = 缺省' */
    // 对象整体覆盖（与 mergeTrees 语义一致）
    expect(merged.formatBindings).toEqual({ 'builtin-json': ['注释'] });

    // 整层写入（对话框全量表单）= 全键覆盖，符合"更新会话默认值"语义
    const fullKind = defaultCompareRulesState();
    const mergedFull = mergeRuleLayers([global, fullKind]);
    expect(mergedFull.caseUnimportant).toBe(false); /* '整层全键覆盖' */
  });

  it('生效元素逐层回落（per-format 绑定，非整体覆盖）', () => {
    const global = {
      ...defaultCompareRulesState(),
      formatBindings: { 'builtin-cpp': ['注释', '字符串'] },
    };
    const kind = { ...defaultCompareRulesState(), formatBindings: { 'builtin-json': ['注释'] } };
    // cpp 在 kind 层无绑定 -> 回落 global
    expect(effectiveElementsAcrossLayers([global, kind], 'builtin-cpp')).toEqual([
      '注释',
      '字符串',
    ]);
    // json 取 kind 层
    expect(effectiveElementsAcrossLayers([global, kind], 'builtin-json')).toEqual(['注释']);
    // 无任何层定义 -> 空
    expect(effectiveElementsAcrossLayers([defaultCompareRulesState()], 'builtin-cpp')).toEqual([]);
    // null/undefined 层跳过
    expect(effectiveElementsAcrossLayers([null, undefined, global], 'builtin-cpp')).toEqual([
      '注释',
      '字符串',
    ]);
  });

  it('会话层覆盖后回落链：session > kind > global（FR-SES-006）', () => {
    const global = { ...defaultCompareRulesState(), formatBindings: { 'builtin-cpp': ['注释'] } };
    const kind = {
      ...defaultCompareRulesState(),
      formatBindings: { 'builtin-cpp': ['注释', '字符串'] },
    };
    const session = { ...defaultCompareRulesState(), formatBindings: { 'builtin-cpp': [] } };
    expect(effectiveElementsAcrossLayers([global, kind, session], 'builtin-cpp')).toEqual(
      [],
    ); /* '会话层显式清空' */
    const merged = mergeRuleLayers([global, kind, session]);
    expect(merged.formatBindings).toEqual({ 'builtin-cpp': [] });
    expect(toRulesSpec(merged, 'builtin-cpp').unimportantElements).toEqual([]);
  });
});
