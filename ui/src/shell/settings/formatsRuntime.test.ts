import { afterEach, describe, expect, it, vi } from 'vitest';
import { sessionService } from '../../session/service';
import type { SettingsTree } from '../../session/model';
import { effectiveFormatsFromSettings } from './formatsRuntime';
import { BUILTIN_FORMAT_CATALOG } from './formatsModel';

describe('settings/formatsRuntime（生效格式列表的运行时读取）', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('读全局 settings 的 fileFormats 组：内置覆盖 + 自定义追加', () => {
    vi.spyOn(sessionService, 'getGlobalSettings').mockReturnValue({
      fileFormats: {
        builtinOverrides: { 'builtin-picture': { enabled: false } },
        custom: [{ ...BUILTIN_FORMAT_CATALOG[0], id: 'fmt-x', masks: ['*.x'] }],
      },
    } as SettingsTree);

    const formats = effectiveFormatsFromSettings();
    expect(formats.find((f) => f.id === 'builtin-picture')!.enabled).toBe(false);
    expect(formats[formats.length - 1].id).toBe('fmt-x');
  });

  it('设置层未初始化/损坏时回落内置表，不抛错', () => {
    vi.spyOn(sessionService, 'getGlobalSettings').mockReturnValue({} as SettingsTree);
    expect(effectiveFormatsFromSettings().map((f) => f.id)).toEqual(
      BUILTIN_FORMAT_CATALOG.map((f) => f.id),
    );

    vi.spyOn(sessionService, 'getGlobalSettings').mockImplementation(() => {
      throw new Error('settings 未初始化');
    });
    expect(effectiveFormatsFromSettings().length).toBe(BUILTIN_FORMAT_CATALOG.length);
  });
});
