/**
 * 编码选择（ENC-001 / FR-TXT-013 的 UI 侧）。
 *
 * 引擎是编码权威：text/open 带 `encoding` 参数，引擎按标签解码
 * （auto = BOM → 合法 UTF-8 → 统计检测），保存按打开时生效的编码 + 原 BOM
 * 回写。UI 只把用户选择送上协议面，并显示引擎回传的生效编码。
 *
 * 不在客户端解码：客户端解码只影响显示，引擎仍按自己的口径保存，
 * 两边会分叉（R3-15 之前的形态：选择器切了编码，落盘字节却不变）。
 */

export interface EncodingChoice {
  /** 选择器 id（UI 状态存储用）；'auto' 走引擎自动检测 */
  id: EncodingId;
  /** 用户可见名 */
  label: string;
  /** 引擎编码标签（WHATWG 口径）；null = 自动检测 */
  engineLabel: string | null;
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
  { id: 'auto', label: '自动检测（默认）', engineLabel: null },
  { id: 'utf-8', label: 'UTF-8', engineLabel: 'utf-8' },
  { id: 'utf-16le', label: 'UTF-16 LE', engineLabel: 'utf-16le' },
  { id: 'utf-16be', label: 'UTF-16 BE', engineLabel: 'utf-16be' },
  { id: 'gbk', label: 'GBK（简体中文）', engineLabel: 'gbk' },
  { id: 'gb18030', label: 'GB18030', engineLabel: 'gb18030' },
  { id: 'big5', label: 'Big5（繁体中文）', engineLabel: 'big5' },
  { id: 'shift_jis', label: 'Shift-JIS（日文）', engineLabel: 'shift_jis' },
  { id: 'windows-1252', label: 'Latin-1 / Windows-1252', engineLabel: 'windows-1252' },
];

export function encodingById(id: string): EncodingChoice {
  return ENCODING_CHOICES.find((e) => e.id === id) ?? ENCODING_CHOICES[0];
}

/** text/open 的 encoding 参数：auto 省略键（与不带该字段的旧载荷字节一致） */
export function encodingParam(id: EncodingId): string | undefined {
  return encodingById(id).engineLabel ?? undefined;
}

/** 引擎回传的生效编码 -> 选择器可见名（未知标签原样显示，不猜） */
export function encodingDisplayName(engineLabel: string): string {
  const hit = ENCODING_CHOICES.find(
    (e) => e.engineLabel !== null && e.engineLabel.toLowerCase() === engineLabel.toLowerCase(),
  );
  return hit ? hit.label : engineLabel;
}
