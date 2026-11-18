import type { TextEditOp, TextEditParams } from './types';

/**
 * `text/edit` 参数构造（A3）。
 *
 * 显式写出两侧 EOF 标记：`null` = 本次不改变该侧末行行尾符。协议把这两个字段
 * 定义为**必填**（值可为 null），就是为了让"不改"和"改成 false"在类型层面
 * 无法混淆——调用方漏写即编译失败，而不是凭默认值猜。
 */
export function textEditParams(
  sessionId: string,
  edits: TextEditOp[],
  opts: { leftTrailingEol?: boolean | null; rightTrailingEol?: boolean | null } = {},
): TextEditParams {
  return {
    sessionId,
    edits,
    leftTrailingEol: opts.leftTrailingEol ?? null,
    rightTrailingEol: opts.rightTrailingEol ?? null,
  };
}
