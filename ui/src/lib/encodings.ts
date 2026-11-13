/**
 * 编码选择与 BOM 处理（ENC-001 / FR-TXT-013 编码覆盖的 UI 侧）。
 *
 * 会话级编码选择：切换后由视图触发"重新加载重 diff"（会话级语义，不落盘）。
 * BOM 处理与引擎对齐（engine/ai-vfs read_file_text：UTF-8 BOM 剥离后解码）：
 * - auto：按 BOM 嗅探（UTF-8 / UTF-16LE / UTF-16BE），无 BOM 按 UTF-8（引擎 P0 口径）；
 * - 显式编码：按所选 TextDecoder 标签解码，BOM 一律剥离（不进入比较内容）。
 *
 * 引擎 P0 仅支持 UTF-8（含 BOM 剥离）；其余编码为 UI 侧解码能力，字节通道
 * （引擎 text/lines 携带编码或 P10 编码检测链）就绪前仅对客户端持有的字节生效。
 */

export interface EncodingChoice {
  /** 选择器 id（UI 状态存储用）；'auto' 走 BOM 嗅探 + UTF-8 回退 */
  id: EncodingId;
  /** 用户可见名 */
  label: string;
  /** TextDecoder 标签；null = 仅 BOM 嗅探（auto） */
  decoderLabel: string | null;
}

export type EncodingId =
  | 'auto'
  | 'utf-8'
  | 'utf-16le'
  | 'utf-16be'
  | 'gbk'
  | 'gb18030'
  | 'big5'
  | 'shift_jis'
  | 'windows-1252';

export const ENCODING_CHOICES: EncodingChoice[] = [
  { id: 'auto', label: '自动检测（默认）', decoderLabel: null },
  { id: 'utf-8', label: 'UTF-8', decoderLabel: 'utf-8' },
  { id: 'utf-16le', label: 'UTF-16 LE', decoderLabel: 'utf-16le' },
  { id: 'utf-16be', label: 'UTF-16 BE', decoderLabel: 'utf-16be' },
  { id: 'gbk', label: 'GBK（简体中文）', decoderLabel: 'gbk' },
  { id: 'gb18030', label: 'GB18030', decoderLabel: 'gb18030' },
  { id: 'big5', label: 'Big5（繁体中文）', decoderLabel: 'big5' },
  { id: 'shift_jis', label: 'Shift-JIS（日文）', decoderLabel: 'shift_jis' },
  { id: 'windows-1252', label: 'Latin-1 / Windows-1252', decoderLabel: 'windows-1252' },
];

export function encodingById(id: string): EncodingChoice {
  return ENCODING_CHOICES.find((e) => e.id === id) ?? ENCODING_CHOICES[0];
}

export type BomKind = 'utf-8' | 'utf-16le' | 'utf-16be' | null;

/** BOM 嗅探（字节前缀；引擎 strip_bom 对齐点：UTF-8 EF BB BF） */
export function detectBom(bytes: Uint8Array): BomKind {
  if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    return 'utf-8';
  }
  if (bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xfe) return 'utf-16le';
  if (bytes.length >= 2 && bytes[0] === 0xfe && bytes[1] === 0xff) return 'utf-16be';
  return null;
}

/** BOM 字节长度（剥离用；与 detectBom 配对） */
export function bomLength(kind: BomKind): number {
  return kind === null ? 0 : kind === 'utf-8' ? 3 : 2;
}

/**
 * 解码字节为文本：先剥 BOM（所有编码统一剥离，BOM 不进入比较内容——
 * 与引擎 UTF-8 BOM 剥离语义对齐），再按所选/auto 编码解码。
 * 解码失败（如 GBK 选错到二进制）按 TextDecoder fatal 语义抛 TypeError，
 * 由调用方给出"编码不匹配"提示。
 */
export function decodeBytes(bytes: Uint8Array, id: EncodingId): string {
  const bom = detectBom(bytes);
  let label: string;
  if (id === 'auto') {
    label = bom ?? 'utf-8';
  } else {
    label = encodingById(id).decoderLabel ?? 'utf-8';
  }
  const start = bomLength(bom);
  // fatal=true：错误编码快速失败（GPT 式静默替换会掩盖编码选择错误）
  const decoder = new TextDecoder(label, { fatal: true });
  return decoder.decode(bytes.subarray(start));
}

/** 编码是否为引擎 P0 直接支持的（UI 显示"引擎直读"提示用） */
export function isEngineNative(id: EncodingId): boolean {
  return id === 'auto' || id === 'utf-8';
}
