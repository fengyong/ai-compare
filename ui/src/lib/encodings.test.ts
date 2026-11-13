import { describe, expect, it } from 'vitest';
import {
  ENCODING_CHOICES,
  bomLength,
  decodeBytes,
  detectBom,
  encodingById,
  isEngineNative,
  type EncodingId,
} from './encodings';

describe('encodings：编码选择与 BOM 处理（ENC-001）', () => {
  it('BOM 嗅探：UTF-8 / UTF-16LE / UTF-16BE / 无 BOM', () => {
    expect(detectBom(new Uint8Array([0xef, 0xbb, 0xbf, 0x61]))).toBe('utf-8');
    expect(detectBom(new Uint8Array([0xff, 0xfe, 0x41, 0x00]))).toBe('utf-16le');
    expect(detectBom(new Uint8Array([0xfe, 0xff, 0x00, 0x41]))).toBe('utf-16be');
    expect(detectBom(new Uint8Array([0x61, 0x62]))).toBeNull();
    expect(bomLength('utf-8')).toBe(3);
    expect(bomLength('utf-16le')).toBe(2);
    expect(bomLength(null)).toBe(0);
  });

  it('auto：UTF-8 BOM 剥离后解码（与引擎 read_file_text 语义对齐）', () => {
    const bytes = new Uint8Array([0xef, 0xbb, 0xbf, ...new TextEncoder().encode('hello')]);
    expect(decodeBytes(bytes, 'auto')).toBe('hello');
  });

  it('auto：无 BOM 按 UTF-8（引擎 P0 口径）', () => {
    expect(decodeBytes(new TextEncoder().encode('中文'), 'auto')).toBe('中文');
  });

  it('显式 GBK 解码（编码覆盖生效路径）', () => {
    // "你好" 的 GBK 字节
    const bytes = new Uint8Array([0xc4, 0xe3, 0xba, 0xc3]);
    expect(decodeBytes(bytes, 'gbk')).toBe('你好');
  });

  it('UTF-16LE 带显式编码选择：BOM 仍剥离', () => {
    const bytes = new Uint8Array([0xff, 0xfe, 0x41, 0x00, 0x42, 0x00]);
    expect(decodeBytes(bytes, 'utf-16le')).toBe('AB');
  });

  it('编码不匹配 → 快速失败（fatal 语义，不静默替换）', () => {
    const bytes = new Uint8Array([0xc4, 0xe3, 0xba, 0xc3]); // GBK "你好"
    // 同一字节序列按 UTF-8 fatal 解码（C4E3 非合法 UTF-8 序列）→ 抛错
    expect(() => decodeBytes(bytes, 'utf-8')).toThrow();
  });

  it('Latin-1 / windows-1252 解码', () => {
    // 0xE9 = é（windows-1252）
    expect(decodeBytes(new Uint8Array([0xe9]), 'windows-1252')).toBe('é');
  });

  it('常用集注册表：含 auto + 8 种显式编码，标签均可用', () => {
    expect(ENCODING_CHOICES.length).toBe(9);
    for (const e of ENCODING_CHOICES) {
      if (e.decoderLabel) {
        expect(() => new TextDecoder(e.decoderLabel!)).not.toThrow();
      }
    }
  });

  it('encodingById：未知 id 回退 auto', () => {
    expect(encodingById('nonsense').id).toBe('auto');
    expect(encodingById('gbk').decoderLabel).toBe('gbk');
  });

  it('isEngineNative：仅 auto/utf-8 为引擎 P0 直读', () => {
    expect(isEngineNative('auto')).toBe(true);
    expect(isEngineNative('utf-8')).toBe(true);
    expect(isEngineNative('gbk')).toBe(false);
    expect(isEngineNative('utf-16le' as EncodingId)).toBe(false);
  });
});
