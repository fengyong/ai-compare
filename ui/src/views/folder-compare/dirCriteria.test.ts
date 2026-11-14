import { describe, expect, it } from 'vitest';
import { DEFAULT_DIR_CRITERIA } from '../../protocol/types';
import {
  CONTENT_METHODS,
  criteriaFromCompareGroup,
  effectiveDirCriteria,
  effectiveDirRules,
  isContentMethod,
  isContentMode,
  rulesFromCompareRulesGroup,
} from './dirCriteria';

/**
 * R3-16：设置页「比较」组 → dir/open criteria 的映射。
 * 此前视图硬编码 DEFAULT_DIR_CRITERIA，设置写了也不生效。
 */
describe('criteriaFromCompareGroup', () => {
  it('三种内容比较方法都被接受（FR-DIR-011）', () => {
    expect(CONTENT_METHODS).toEqual(['crc', 'binary', 'rulesBased']);
    for (const m of CONTENT_METHODS) {
      expect(isContentMethod(m)).toBe(true);
      expect(criteriaFromCompareGroup({ contentMethod: m }).contentMethod).toBe(m);
    }
  });

  it('空组 = 全默认', () => {
    expect(criteriaFromCompareGroup({})).toEqual(DEFAULT_DIR_CRITERIA);
  });

  it('非法/缺失值回落默认，不把 undefined 传进引擎（serde 会整份拒绝）', () => {
    expect(isContentMethod('sha256')).toBe(false);
    expect(isContentMethod(undefined)).toBe(false);
    expect(isContentMode('sometimes')).toBe(false);
    expect(
      criteriaFromCompareGroup({
        contentMethod: 'sha256',
        contentCompare: 'sometimes',
        mtimeToleranceSec: -1,
      }),
    ).toEqual(DEFAULT_DIR_CRITERIA);
  });

  it('contentCompare 三态映射（never/auto/always）', () => {
    for (const mode of ['never', 'auto', 'always'] as const) {
      expect(isContentMode(mode)).toBe(true);
      expect(criteriaFromCompareGroup({ contentCompare: mode }).contentCompare).toBe(mode);
    }
  });

  it('mtimeToleranceSec 取整，拒绝负数/NaN/Infinity/非数字', () => {
    expect(criteriaFromCompareGroup({ mtimeToleranceSec: 5.9 }).mtimeToleranceSec).toBe(5);
    for (const bad of [-1, NaN, Infinity, '3', null]) {
      expect(criteriaFromCompareGroup({ mtimeToleranceSec: bad }).mtimeToleranceSec).toBe(
        DEFAULT_DIR_CRITERIA.mtimeToleranceSec,
      );
    }
  });

  it('只覆盖三键，其余判据字段保持默认（不丢 ignoreDstHour/compareCase）', () => {
    const c = criteriaFromCompareGroup({ contentMethod: 'rulesBased' });
    expect(c.ignoreDstHour).toBe(DEFAULT_DIR_CRITERIA.ignoreDstHour);
    expect(c.compareCase).toBe(DEFAULT_DIR_CRITERIA.compareCase);
    expect(c.unixMeta).toBe(DEFAULT_DIR_CRITERIA.unixMeta);
  });
});

describe('rulesFromCompareRulesGroup', () => {
  it('空组 = 未配置 → undefined（引擎默认重要性）', () => {
    expect(rulesFromCompareRulesGroup({})).toBeUndefined();
  });

  it('开关映射到 RulesSpec；元素集为空（目录内文件格式各异，无法统一绑定）', () => {
    expect(
      rulesFromCompareRulesGroup({ caseUnimportant: true, leadingWsUnimportant: true }),
    ).toEqual({
      leadingWsUnimportant: true,
      embeddedWsUnimportant: false,
      trailingWsUnimportant: false,
      caseUnimportant: true,
      eolStyleUnimportant: true,
      encodingUnimportant: true,
      unimportantElements: [],
    });
  });

  it('部分覆盖不丢其余默认开关', () => {
    const r = rulesFromCompareRulesGroup({ eolStyleUnimportant: false });
    expect(r?.eolStyleUnimportant).toBe(false);
    expect(r?.encodingUnimportant).toBe(true);
  });
});

describe('生效设置读取（global → kind → session）', () => {
  it('设置层未初始化时安全回落，不抛异常阻断会话打开', () => {
    expect(effectiveDirCriteria()).toEqual(DEFAULT_DIR_CRITERIA);
    expect(effectiveDirRules()).toBeUndefined();
  });
});
