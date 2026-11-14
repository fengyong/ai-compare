import type { TextSaveEntry, TextSaveParams, TextSaveResult } from '../../protocol/types';
import { putTextSession, type TextSessionModel } from './textModel';

/**
 * 文本落盘控制器（R3-04，FR-TXT-020）。
 *
 * 逐侧显式保存而非 `side: 'both'`：引擎的 both 语义依赖引擎侧"未保存编辑"
 * 标记，而编码切换会重开会话（标记随之清零），此时 UI 仍持有编辑内容。
 * 由 UI 的脏标记决定写哪侧，避免"以为保存了其实没写"。
 */

export type SaveSide = 'left' | 'right';

export interface SaveRequest {
  side: SaveSide;
  /** 另存为目标；省略 = 写回该侧当前路径 */
  path?: string;
}

export interface TextSaveIO {
  save(params: TextSaveParams): Promise<TextSaveResult | null>;
}

export interface TextSaveOutcome {
  saved: TextSaveEntry[];
  /** 引擎无 text/save 能力：调用方必须提示"未保存"，不得当成已保存 */
  unsupported: boolean;
  /** 失败信息；已落盘侧仍留在 saved 中（部分成功必须可见） */
  error: string | null;
}

/** 脏侧 → 保存请求（顺序固定左先右后，与 BC 逐侧保存一致） */
export function saveRequestsFor(dirty: { left: boolean; right: boolean }): SaveRequest[] {
  return (['left', 'right'] as const).filter((s) => dirty[s]).map((side) => ({ side }));
}

export async function runTextSave(
  sessionId: string,
  requests: readonly SaveRequest[],
  io: TextSaveIO,
): Promise<TextSaveOutcome> {
  const saved: TextSaveEntry[] = [];
  for (const req of requests) {
    const target =
      req.path === undefined
        ? {}
        : req.side === 'left'
          ? { leftPath: req.path }
          : { rightPath: req.path };
    let res: TextSaveResult | null;
    try {
      res = await io.save({ sessionId, side: req.side, ...target });
    } catch (err) {
      return { saved, unsupported: false, error: err instanceof Error ? err.message : String(err) };
    }
    if (res === null) return { saved, unsupported: true, error: null };
    saved.push(...res.saved);
  }
  return { saved, unsupported: false, error: null };
}

/**
 * 落盘结果回写会话模型：只清除真正落盘的侧，另存为把该侧路径改指新目标
 * （下次默认保存不再写原文件）。视图工具栏与壳级关闭守卫共用这一份语义。
 */
export function applySavedEntries(
  model: TextSessionModel | undefined,
  saved: readonly TextSaveEntry[],
): void {
  if (!model) return;
  for (const e of saved) {
    model.modifiedSides[e.side] = false;
    if (e.side === 'left') model.leftPath = e.path;
    else model.rightPath = e.path;
  }
  putTextSession(model);
}
