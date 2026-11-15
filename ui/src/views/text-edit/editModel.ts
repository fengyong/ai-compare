/**
 * 单窗格文本编辑模型（D2 / FR-EDIT-001 第一阶段）。
 *
 * 设计要点（与"复用双栏"的区别）：
 * - **引擎是内容与保存的权威**：编辑以 `text/edit` 增量提交（行区间替换），
 *   保存以 `text/save side=left` 落盘；UI 不自行编码/写文件。
 * - 本模块只做"编辑器文档 ⇄ 引擎行表"的换算与收敛，不碰 DOM/React，可单测。
 *
 * ## 行数组与文本文档的可逆换算（坑点）
 *
 * `text/lines` 返回的行不含行尾符；CM6 文档是 `lines.join('\n')`。反向换算
 * 必须与引擎 `LineTable::from_text` 完全一致，否则行数对不上会触发重放/分叉：
 *
 * - `docLinesOf(text)`：`''` → 0 行；否则 `split('\n')`（`'a\nb\n'` → 3 行，
 *   末行为空行）。
 * - `linesToEditText(lines)`：末行为空时必须补一个 `'\n'`，否则引擎会把
 *   `['']`/`['a','']` 解析成 0/1 行（`from_text` 不把结尾换行当作新行）。
 *
 * ## 已知边界：文件末行换行
 *
 * 引擎行表无法表达"末行为空且无行尾符"，因此**在文件末尾新增空行**时会被写成
 * 一个带行尾符的空行（`"a\nb"` 按一次 Enter → `"a\nb\n\n"`）。这是协议
 * 表达力的边界，不是本模块的换算错误：行数始终一致，保存不会丢内容，
 * 且每次回车恰好追加一个换行。视图层不对此做遮掩。
 */

import type { TextEditOp, TextEditResult } from '../../protocol/types';

/** 单窗格编辑文档（会话级缓存；标签切换不丢编辑，与 textModel 同构） */
export interface EditDoc {
  sessionId: string;
  /** 当前落盘目标（另存为后改指） */
  path: string;
  /** 显示名（basename；随另存为更新） */
  name: string;
  /** 打开时引擎检测到的编码标签（保存由引擎按此回写） */
  encoding: string;
  /** UI 侧真值：当前行内容（不含行尾符） */
  lines: string[];
  /** 引擎已确认收到的行内容（收敛基准；保存前必须与 lines 相等） */
  engineSnapshot: string[];
  /** 上次落盘时的内容（判脏基准） */
  savedLines: string[];
  /** 引擎会话版本号（text/edit 应答回填） */
  rev: number;
  /** 收敛失败（UI 与引擎内容无法对齐；保存必须被阻止，不得写陈旧内容） */
  diverged: boolean;
}

/**
 * 文本文档 → 行数组（与 `LineTable::from_text` 的分行口径一致）。
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

/** 行级最小替换区间（公共前缀/后缀裁剪；无变化返回 null） */
export interface LineEditSpan {
  start: number;
  /** 被替换区间的结束（exclusive，旧行坐标） */
  end: number;
  /** 替换文本（`text/edit` 口径） */
  text: string;
}

/**
 * 由"引擎当前行数组 → 目标行数组"计算一次行区间替换。
 *
 * 返回的区间语义与引擎一致：`[start, end)` 换成 `text` 解析出的行。
 * 公共前后缀不改动，故一次编辑只会重写真正变化的中段（引擎侧只对这一段落行表）。
 */
export function computeLineEdit(
  from: readonly string[],
  to: readonly string[],
): LineEditSpan | null {
  const maxStart = Math.min(from.length, to.length);
  let start = 0;
  while (start < maxStart && from[start] === to[start]) start++;
  let endFrom = from.length;
  let endTo = to.length;
  while (endFrom > start && endTo > start && from[endFrom - 1] === to[endTo - 1]) {
    endFrom--;
    endTo--;
  }
  if (start === endFrom && start === endTo) return null;
  return { start, end: endFrom, text: linesToEditText(to.slice(start, endTo)) };
}

/** 区间 → `text/edit` 载荷（单侧固定 left） */
export function editOpFor(span: LineEditSpan): TextEditOp {
  return { side: 'left', start: span.start, end: span.end, text: span.text };
}

/** 整侧重放（收敛兜底：把引擎行表整体改写为 UI 内容） */
export function replayOpFor(lines: readonly string[]): TextEditOp {
  return { side: 'left', start: 0, text: linesToEditText(lines) };
}

/** 收敛结果 */
export type SyncOutcome = 'ok' | 'unsupported' | 'mismatch';

export interface EditSyncIO {
  /** text/edit；null = 引擎无此能力（编辑器必须转只读，不得允许"保存假成功"） */
  edit(edits: TextEditOp[]): Promise<TextEditResult | null>;
}

/** 收敛最多尝试的轮数（每轮 = 一次增量 + 可选一次整侧重放） */
const MAX_SYNC_ROUNDS = 4;

/**
 * 把引擎会话内容收敛到 UI 真值（幂等；编辑/保存前调用）。
 *
 * 每轮以 `engineSnapshot` 为基算最小增量，提交后用应答行数校验——行数不符
 * （例如末行空行口径差异）时整侧重放一次；仍不符即判定分叉并**停止**，
 * 由调用方阻止保存（宁可报错，也不把陈旧内容写盘）。
 */
export async function syncDocToEngine(doc: EditDoc, io: EditSyncIO): Promise<SyncOutcome> {
  return queueDocOp(doc, () => runSync(doc, io));
}

/**
 * 每文档一条**引擎操作队列**：同一文档的所有引擎写操作（增量同步、收敛、落盘）
 * 必须串行。
 *
 * 只串行同步还不够——"收敛后落盘"如果中间能被别的写入穿插，落盘内容与
 * `savedLines` 记录就会错配（文件是新的、模型以为存的是旧的，或反之）。
 * 因此队列对所有 `text/edit`/`text/save` 调用统一排他：入队即原子。
 *
 * 队列不吞异常（出错的那次调用照常 reject），但链本身永远向下一步推进，
 * 一次失败不会把后续操作永久卡死。
 */
export function queueDocOp<T>(doc: EditDoc, op: () => Promise<T>): Promise<T> {
  const prev = docQueues.get(doc) ?? Promise.resolve();
  const next = prev.then(op, op);
  docQueues.set(
    doc,
    next.then(
      () => undefined,
      () => undefined,
    ),
  );
  return next;
}

/** 每文档一条操作链（WeakMap：文档被回收即释放） */
const docQueues = new WeakMap<EditDoc, Promise<void>>();

/**
 * 在**已持有文档锁**的临界区内执行收敛（保存等需要"收敛+落盘"原子的调用方用）。
 *
 * 直接调用 `syncDocToEngine` 会二次入队 → 自我等待。因此保存流程必须用这个
 * 内部版本，并且只能在 `queueDocOp` 回调里调用。
 */
export async function convergeDocInLock(doc: EditDoc, io: EditSyncIO): Promise<SyncOutcome> {
  return runSync(doc, io);
}

async function runSync(doc: EditDoc, io: EditSyncIO): Promise<SyncOutcome> {
  doc.diverged = false;
  for (let round = 0; round < MAX_SYNC_ROUNDS; round++) {
    if (linesEqual(doc.engineSnapshot, doc.lines)) return 'ok';
    const span = computeLineEdit(doc.engineSnapshot, doc.lines);
    if (span) {
      // io.edit 的真实错误（断连/区间非法）向上抛：调用方必须看见，不得当成"已同步"
      const res = await io.edit([editOpFor(span)]);
      if (res === null) return 'unsupported';
      doc.rev = res.rev;
      if (res.leftLineCount === doc.lines.length) {
        doc.engineSnapshot = [...doc.lines];
        return 'ok';
      }
    }
    // 行数不符：整侧重放一次（把引擎行表直接改写为 UI 内容）
    const res2 = await io.edit([replayOpFor(doc.lines)]);
    if (res2 === null) return 'unsupported';
    doc.rev = res2.rev;
    if (res2.leftLineCount === doc.lines.length) {
      doc.engineSnapshot = [...doc.lines];
      return 'ok';
    }
    // 重放后仍不符：内容口径不一致，停止并标记（保存将被阻止）
    doc.diverged = true;
    return 'mismatch';
  }
  doc.diverged = true;
  return 'mismatch';
}

/**
 * 按落盘路径找回**有未落盘编辑**的文档（崩溃重放/重新打开时保住用户输入）。
 *
 * 场景：引擎崩溃后重放会给会话换新句柄，视图按新句柄重新 open 同一文件
 * （引擎内容回到文件态），此时旧文档里的未保存编辑仍在 UI 缓存中——按路径
 * 认领回来，再把它重新收敛进新会话，避免"崩溃后编辑凭空消失"。
 */
export function findDirtyEditDocByPath(
  path: string,
  excludeSessionId?: string,
): EditDoc | undefined {
  for (const doc of cache.values()) {
    if (doc.sessionId === excludeSessionId) continue;
    if (doc.path === path && isDocDirty(doc)) return doc;
  }
  return undefined;
}

/** 是否有未落盘编辑（与引擎状态无关，只看 UI 真值） */
export function isDocDirty(doc: EditDoc): boolean {
  return !linesEqual(doc.lines, doc.savedLines);
}

/** 视图状态栏文本（纯函数，便于测试） */
export interface DocStatus {
  lineCount: number;
  encoding: string;
  dirty: boolean;
}

export function docStatus(doc: EditDoc): DocStatus {
  return { lineCount: doc.lines.length, encoding: doc.encoding, dirty: isDocDirty(doc) };
}

// ---------------------------------------------------------------------------
// 会话级缓存（与 views/text-compare/textModel 同构：标签切换不丢编辑）
// ---------------------------------------------------------------------------

const CACHE_LIMIT = 32;
const cache = new Map<string, EditDoc>();

export function getEditDoc(sessionId: string): EditDoc | undefined {
  return cache.get(sessionId);
}

export function putEditDoc(doc: EditDoc): void {
  cache.delete(doc.sessionId);
  cache.set(doc.sessionId, doc);
  if (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
}

export function deleteEditDoc(sessionId: string): void {
  cache.delete(sessionId);
}

/** 由打开结果构建初始文档（引擎行内容即初始快照，未修改） */
export function newEditDoc(init: {
  sessionId: string;
  path: string;
  name: string;
  encoding: string;
  lines: string[];
  rev?: number;
}): EditDoc {
  const lines = [...init.lines];
  return {
    sessionId: init.sessionId,
    path: init.path,
    name: init.name,
    encoding: init.encoding,
    lines,
    engineSnapshot: [...lines],
    savedLines: [...lines],
    rev: init.rev ?? 0,
    diverged: false,
  };
}
