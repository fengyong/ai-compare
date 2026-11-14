import { describe, expect, it } from 'vitest';
import {
  ENCODING_CHOICES,
  encodingById,
  encodingDisplayName,
  encodingParam,
  type EncodingId,
} from './encodings';

describe('encodings：编码选择上协议面（ENC-001 / R3-15）', () => {
  it('常用集注册表：auto + 8 种显式编码，标签均为 WHATWG 口径', () => {
    expect(ENCODING_CHOICES.length).toBe(9);
    for (const e of ENCODING_CHOICES) {
      if (e.engineLabel) {
        // 引擎用 encoding_rs 的 WHATWG 标签表，与 TextDecoder 同一套标签
        expect(() => new TextDecoder(e.engineLabel!)).not.toThrow();
      }
    }
  });

  it('encodingById：未知 id 回退 auto', () => {
    expect(encodingById('nonsense').id).toBe('auto');
    expect(encodingById('gbk').engineLabel).toBe('gbk');
  });

  it('encodingParam：auto 省略键，显式编码送引擎标签', () => {
    expect(encodingParam('auto')).toBeUndefined();
    expect(encodingParam('gb18030')).toBe('gb18030');
    expect(encodingParam('utf-16le')).toBe('utf-16le');
    expect(encodingParam('windows-1252')).toBe('windows-1252');
    expect(encodingParam('nonsense' as EncodingId)).toBeUndefined();
  });

  it('encodingDisplayName：引擎标签大小写不敏感映射到可见名', () => {
    expect(encodingDisplayName('UTF-8')).toBe('UTF-8');
    expect(encodingDisplayName('GBK')).toBe('GBK（简体中文）');
    expect(encodingDisplayName('UTF-16LE')).toBe('UTF-16 LE');
    expect(encodingDisplayName('Shift_JIS')).toBe('Shift-JIS（日文）');
    // 引擎支持但选择器未列出的编码：原样显示，不猜成别的
    expect(encodingDisplayName('EUC-JP')).toBe('EUC-JP');
  });
});
