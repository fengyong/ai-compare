/**
 * 单窗格编辑落盘（D2 / FR-EDIT-001，FR-TXT-020 续）。
 *
 * 视图工具栏与壳级关闭守卫**共用**这一份语义，避免两条保存路径出现分叉：
 * 1. 先 `text/edit` 把 UI 内容收敛到引擎（引擎是落盘权威，未收敛就保存会写陈旧
 *    内容——这是"假报保存成功"的根源，必须在这里挡住）；
 * 2. 再 `text/save side=left` 落盘（编码/BOM/行尾由引擎按打开时生效的结果回写，
 *    `backup` 缺省 true → 覆盖前复制 `<path>.bak`）；
 * 3. 只用引擎应答回写模型：`saved[].path` 才是真实目标（另存为会改指）。
 */

import type { TextSaveResult } from '../../protocol/types';
import * as engine from '../../ipc/engineProcess';
import { RpcError } from '../../protocol/client';
import { basename } from '../../lib/env';
import { S } from '../../strings';
import { syncDocToEngine, type EditDoc } from './editModel';

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
  /** ok=false 时的用户可见原因 */
  message: string;
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
 */
export async function saveEditDoc(
  doc: EditDoc,
  opts: SaveEditOptions = {},
): Promise<SaveEditResult> {
  /** 落盘前的目标（判断是否需要改指标签/标题） */
  const previous = doc.path;
  const failed = (message: string): SaveEditResult => ({
    ok: false,
    path: doc.path,
    retargeted: false,
    message,
  });

  // 1) 收敛引擎内容
  let outcome: Awaited<ReturnType<typeof syncDocToEngine>>;
  try {
    outcome = await syncDocToEngine(doc, {
      edit: (edits) => engine.editText({ sessionId: doc.sessionId, edits }),
    });
  } catch (err) {
    return failed(describeSaveError(err));
  }
  if (outcome === 'unsupported') return failed(S.edit.editUnsupported);
  if (outcome === 'mismatch') return failed(S.edit.diverged);

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
  doc.savedLines = [...doc.lines];
  return {
    ok: true,
    path: entry.path,
    backupPath: entry.backupPath,
    retargeted,
    message: '',
  };
}

/** 成功提示（含备份信息，避免用户不知道 .bak 在哪） */
export function savedNotice(r: SaveEditResult): string {
  return r.backupPath
    ? `${S.edit.saveOkBackup}：${r.path} → ${r.backupPath}`
    : `${S.edit.saveOk}：${r.path}`;
}
