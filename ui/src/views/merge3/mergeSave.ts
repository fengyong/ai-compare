import type {
  MergeMarkerStyle,
  MergeOutputMode,
  MergeSaveParams,
  MergeSaveResult,
} from '../../protocol/types';

/**
 * 三向合并输出落盘控制器（R3-20，FR-TMRG-006）。
 *
 * 输出由引擎按会话决策现场合成后直接写目标文件（不经客户端回传文本），
 * 因此非活动标签与大输出同样可保存。目标文件的先前内容按 BC 语义被丢弃，
 * 故写入前必须给出覆盖警示；`backup` 缺省 true 时引擎另存 `<path>.bak`。
 */

export interface MergeSaveOptions {
  mode: MergeOutputMode;
  markerStyle: MergeMarkerStyle;
  /** false = BC 的 `/nobackups`（覆盖前不留备份） */
  backup: boolean;
}

export interface MergeSaveIO {
  save(params: MergeSaveParams): Promise<MergeSaveResult | null>;
  /** 覆盖警示；false = 用户取消，此时不得写入 */
  confirmOverwrite(path: string): Promise<boolean>;
}

export interface MergeSaveOutcome {
  result: MergeSaveResult | null;
  /** 引擎无 mergeSave 能力：调用方必须提示"未保存"，不得当成已保存 */
  unsupported: boolean;
  cancelled: boolean;
  error: string | null;
}

export async function runMergeSave(
  sessionId: string,
  path: string,
  opts: MergeSaveOptions,
  io: MergeSaveIO,
): Promise<MergeSaveOutcome> {
  const target = path.trim();
  if (!(await io.confirmOverwrite(target))) {
    return { result: null, unsupported: false, cancelled: true, error: null };
  }
  let res: MergeSaveResult | null;
  try {
    res = await io.save({
      sessionId,
      path: target,
      mode: opts.mode,
      markerStyle: opts.markerStyle,
      backup: opts.backup,
    });
  } catch (err) {
    return {
      result: null,
      unsupported: false,
      cancelled: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
  if (res === null) {
    return { result: null, unsupported: true, cancelled: false, error: null };
  }
  return { result: res, unsupported: false, cancelled: false, error: null };
}
