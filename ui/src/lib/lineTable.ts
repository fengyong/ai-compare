/**
 * 行数组 ⇄ 文本文档的权威换算（引擎 `LineTable::from_text` / `to_text` 的镜像）。
 *
 * 这是**全 UI 唯一**一份行尾换算，单窗格编辑（`views/text-edit`）与双栏比较
 * （`views/text-compare`）都必须走这里：两处各写一份必然漂移，而漂移的代价是
 * 保存时丢行或引擎反复重放（A1 就是这样丢掉了空行插入）。
 *
 * 引擎口径（`engine/ai-core/src/line_table.rs`）：
 * - `from_text("")` = 0 行；`from_text("a\nb")` = 2 行；`from_text("a\nb\n")` = 2 行
 *   （结尾换行符不产生新行）；`from_text("\n")` = 1 行（一个空行）。
 * - `to_text` = 各行内容 + 各行自己的行尾符（末行可能没有行尾符）。
 *
 * 因此"行数组 → edit text"绝不能直接 `join('\n')`：末行为空时那样会少解析一行，
 * 引擎收到的行表比 UI 少一行 → 行数校验失败 → 重放 → 仍不符 → 判定分叉
 * （比较视图里表现就是"插入的空行保存后消失"）。
 */

/**
 * 文本文档 → 行数组（与 `from_text` 同口径）。
 *
 * `prev` 只用于消除一处**协议表达力导致的歧义**：文档文本 `''` 既可能是
 * "空文件（0 行）"，也可能是"只有一个空行的文件（1 行，内容仅一个行尾符）"——
 * 行数组口径拿不到行尾信息，两者不可区分。此时保持引擎现状（不把 1 行悄悄
 * 变成 0 行），避免"点进编辑器什么都没改就把文件清空"。
 */
export function docLinesOf(text: string, prev?: readonly string[]): string[] {
  if (text !== '') return text.split('\n');
  if (prev && prev.length === 1 && prev[0] === '') return [''];
  return [];
}

/** 行数组 → `text/edit` 的 text（末行为空时补行尾符，否则引擎会少解析一行） */
export function linesToEditText(lines: readonly string[]): string {
  if (lines.length === 0) return '';
  const joined = lines.join('\n');
  return lines[lines.length - 1] === '' ? `${joined}\n` : joined;
}

/** 行数组相等（判脏/收敛判定） */
export function linesEqual(a: readonly string[], b: readonly string[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
