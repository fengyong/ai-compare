/**
 * 单窗格编辑落盘（D2 / FR-EDIT-001，FR-TXT-020 续）。
 *
 * 视图工具栏与壳级关闭守卫**共用**这一份语义，避免两条保存路径出现分叉：
 * 1. 先 `text/edit` 把 UI 内容收敛到引擎（引擎是落盘权威，未收敛就保存会写陈旧
 *    内容——这是"假报保存成功"的根源，必须在这里挡住）；
 * 2. 再 `text/save side=left` 落盘（编码/BOM/行尾由引擎按打开时生效的结果回写，
 *    `backup` 缺省 true → 覆盖前复制 `<path>.bak`）；
 * 3. 只用引擎应答回写模型：`saved[].path` 才是真实目标（另存为会改指）。
 *
 * **A0（保存竞态）**：`savedLines` 只能记录"引擎实际写下去的那一份内容"。
 * 保存是异步的，用户在 `await` 期间仍可继续输入——若在应答回来时用当时的
 * `doc.lines` 当基准，就会把**尚未落盘的新输入**误标为已保存（脏点消失、
 * 关闭不再询问、内容静默丢失）。因此：
 * - 收敛与落盘放在同一把文档锁内（`queueDocOp`），保证"引擎内容 == 快照"
 *   在发起 `text/save` 的那一刻成立；
 * - 落盘前先取 `snapshot`，成功时只把 `savedLines` 设为该快照；
 * - 若期间又有输入（`stillDirty`），保持脏并把新内容继续收敛进引擎。
 */

import type { TextSaveResult } from '../../protocol/types';
import * as engine from '../../ipc/engineProcess';
import { RpcError } from '../../protocol/client';
import { basename } from '../../lib/env';
import { S } from '../../strings';
import {
  convergeDocInLock,
  isDocDirty,
  queueDocOp,
  syncDocToEngine,
  type EditDoc,
} from './editModel';
import { editorSyncIo } from './editIo';

export interface SaveEditOptions {
  /** 覆盖前备份（缺省 true，与引擎默认一致） */
  backup?: boolean;
  /** 另存为目标（省略 = 写回 doc.path） */
  path?: string;
}

export interface SaveEditResult {
  ok: boolean;
  /** 落盘目标（成功时为引擎应答路径；失败时为原路径） */
  path: string;
  backupPath?: string;
  /** 是否把该侧目标改指到了新路径（另存为） */
  retargeted: boolean;
  /** ok=false 时的用户可见原因（ok=true 但保存期间有输入时为提示语） */
  message: string;
  /**
   * 落盘成功，但**保存期间又有新的编辑**：文件已是快照内容，UI 仍脏。
   * 调用方必须保持脏标记；关闭守卫不得据此关闭标签（会丢新输入）。
   */
  stillDirty: boolean;
}

/** RPC 错误 → 用户可读原因（不吞掉服务端消息） */
export function describeSaveError(err: unknown): string {
  if (err instanceof RpcError) {
    // 与 engine/mapping.rs 的错误码口径一致
    if (err.code === 413) return S.edit.errEncoding;
    if (err.code === 412) return `${S.edit.errPathNotFound}（${err.message}）`;
    if (err.code === -32603) return `${S.edit.errIo}（${err.message}）`;
    return `${S.edit.saveFail}：${err.message}`;
  }
  return `${S.edit.saveFail}：${err instanceof Error ? err.message : String(err)}`;
}

/**
 * 落盘单窗格文档（幂等：内容未变时仍会走一次 save，用于"显式保存"语义）。
 *
 * 失败时**不清脏**、不改路径：调用方保留编辑内容并给出原因。
 * 并发调用（连点保存 / 关闭守卫 + Ctrl+S）由文档锁串行：后一次会看到前一次
 * 已落盘的内容，不会写出乱序或重复备份。
 */
export async function saveEditDoc(
  doc: EditDoc,
  opts: SaveEditOptions = {},
): Promise<SaveEditResult> {
  return queueDocOp(doc, () => saveEditDocLocked(doc, opts));
}

/** 在文档锁内执行"收敛 → 快照 → 落盘"（锁内不调用 `syncDocToEngine`，避免自等待） */
async function saveEditDocLocked(doc: EditDoc, opts: SaveEditOptions): Promise<SaveEditResult> {
  /** 落盘前的目标（判断是否需要改指标签/标题） */
  const previous = doc.path;
  const failed = (message: string): SaveEditResult => ({
    ok: false,
    path: doc.path,
    retargeted: false,
    message,
    stillDirty: isDocDirty(doc),
  });

  // 1) 收敛引擎内容（锁内：此后引擎 == doc.lines，直到本次落盘结束）
  let outcome: Awaited<ReturnType<typeof convergeDocInLock>>;
  try {
    outcome = await convergeDocInLock(doc, editorSyncIo(doc.sessionId));
  } catch (err) {
    return failed(describeSaveError(err));
  }
  if (outcome === 'unsupported') return failed(S.edit.editUnsupported);
  if (outcome === 'mismatch') return failed(S.edit.diverged);

  // A0：这份快照就是**即将写盘的内容**。后面的 await 期间用户可能继续输入，
  // 但那些输入不在本次落盘里，绝不能记成"已保存"。
  const snapshot = [...doc.lines];
  // A3：EOF 标记同属"写下去的那一份"，必须与 snapshot 一起记账
  const snapshotTrailing = doc.trailingEol;

  // 2) 落盘
  let res: TextSaveResult | null;
  try {
    res = await engine.saveText({
      sessionId: doc.sessionId,
      side: 'left',
      backup: opts.backup ?? true,
      ...(opts.path === undefined ? {} : { leftPath: opts.path }),
    });
  } catch (err) {
    return failed(describeSaveError(err));
  }
  if (res === null) return failed(S.edit.saveUnsupported);

  const entry = res.saved.find((e) => e.side === 'left');
  if (!entry) return failed(S.edit.saveNoEntry);

  // 3) 只按引擎应答回写（另存为的改指以引擎结果为准）
  const retargeted = entry.path !== previous;
  doc.path = entry.path;
  doc.name = basename(entry.path);
  doc.savedLines = snapshot;
  doc.savedTrailingEol = snapshotTrailing;

  const stillDirty = isDocDirty(doc);
  if (stillDirty) {
    // 保存期间的新输入继续送给引擎（用户不必为"保存竞态"再点一次保存才能看到
    // 引擎追上）；这不影响脏标记——文件确实还不是 UI 的最新内容。
    void syncDocToEngine(doc, editorSyncIo(doc.sessionId)).catch(() => undefined);
  }
  return {
    ok: true,
    path: entry.path,
    backupPath: entry.backupPath,
    retargeted,
    message: stillDirty ? S.edit.saveStillDirty : '',
    stillDirty,
  };
}

/** 成功提示（含备份信息，避免用户不知道 .bak 在哪） */
export function savedNotice(r: SaveEditResult): string {
  if (r.stillDirty) {
    // 不谎报"全部已保存"：文件是快照，最新输入仍未落盘
    return r.backupPath
      ? `${S.edit.saveStillDirty}：${r.path} → ${r.backupPath}`
      : `${S.edit.saveStillDirty}：${r.path}`;
  }
  return r.backupPath
    ? `${S.edit.saveOkBackup}：${r.path} → ${r.backupPath}`
    : `${S.edit.saveOk}：${r.path}`;
}
