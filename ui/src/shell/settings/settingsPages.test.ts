import { describe, expect, it } from 'vitest';
import {
  COMPARE_PAGE,
  COMPARE_RULES_PAGE,
  FILE_FORMATS_PAGE,
  SETTINGS_PAGES,
  fieldDefault,
  pageById,
  readFieldValue,
} from './settingsSchema';

describe('settings/settingsSchema（P10 两页注册）', () => {
  it('SETTINGS_PAGES 注册文件格式页与比较规则页（custom 标记正确）', () => {
    const ids = SETTINGS_PAGES.map((p) => p.id);
    expect(ids).toContain('fileFormats');
    expect(ids).toContain('compareRules');
    expect(pageById('fileFormats')).toBe(FILE_FORMATS_PAGE);
    expect(pageById('compareRules')).toBe(COMPARE_RULES_PAGE);
    expect(FILE_FORMATS_PAGE.custom).toBe('fileFormats');
    expect(COMPARE_RULES_PAGE.custom).toBe('compareRules');
    // 既有三页不受影响
    expect(COMPARE_PAGE.custom).toBeUndefined();
    expect(pageById('compare')).toBe(COMPARE_PAGE);
    expect(pageById('no-such-page')).toBe(COMPARE_PAGE) /* '未知页回落比较页' */;
  });

  it('作用域声明：文件格式 = app（程序级资源），比较规则 = session-scoped（四层）', () => {
    expect(FILE_FORMATS_PAGE.scope).toBe('app');
    expect(COMPARE_RULES_PAGE.scope).toBe('session-scoped');
  });

  it('比较规则页布尔缺省：eol/encoding = true（引擎缺省），其余 false', () => {
    const byKey = (key: string) => COMPARE_RULES_PAGE.fields.find((f) => f.key === key)!;
    expect(fieldDefault(byKey('eolStyleUnimportant'))).toBe(true);
    expect(fieldDefault(byKey('encodingUnimportant'))).toBe(true);
    expect(fieldDefault(byKey('caseUnimportant'))).toBe(false);
    expect(fieldDefault(byKey('leadingWsUnimportant'))).toBe(false);
  });

  it('readFieldValue：层缺失回落字段缺省（default 优先于 type 推导）', () => {
    const eol = COMPARE_RULES_PAGE.fields.find((f) => f.key === 'eolStyleUnimportant')!;
    expect(readFieldValue(undefined, 'compareRules', eol)).toBe(true);
    expect(readFieldValue({ compareRules: { eolStyleUnimportant: false } }, 'compareRules', eol)).toBe(false);
    const custom = FILE_FORMATS_PAGE.fields.find((f) => f.key === 'custom')!;
    expect(readFieldValue({}, 'fileFormats', custom)).toBe('') /* '无 default 的 text 字段回落空串' */;
  });
});
