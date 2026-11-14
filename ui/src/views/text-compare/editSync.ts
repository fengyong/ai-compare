import type { TextEditOp, TextEditResult } from '../../protocol/types';
import { fullSideReplayOp } from './editPatch';

/**
 * 编辑 → 引擎重 diff 的同步状态机（R3-18）。
 *
 * 从视图层抽出以便单测：队列合并、单飞行、分叉收敛与「至多一次整侧重放」
 * 的收敛上限都在这里，视图只负责提供 IO 与渲染。
 */

/** 每个会话一份（会话切换后旧队列不得混入新会话的编辑） */
export type EditSyncState = { queue: TextEditOp[]; flushing: boolean };

export const newEditSyncState = (): EditSyncState => ({ queue: [], flushing: false });

export type EditSyncIO = {
  /** text/edit；null = 引擎无此能力（回落客户端重 diff） */
  edit: (edits: TextEditOp[]) => Promise<TextEditResult | null>;
  /** 当前 UI 两侧行内容：分叉检测与整侧重放的唯一口径 */
  lines: () => { left: readonly string[]; right: readonly string[] };
  /** 采纳引擎权威 sections/summary 并回灌渲染 */
  adopt: (res: TextEditResult) => Promise<void>;
  /** 客户端全量重 diff 并回灌（降级 / 分叉兜底） */
  fallback: () => Promise<void>;
  /** 重放后仍分叉：向用户标注（工具栏） */
  diverged: () => void;
};

/**
 * 入队一批编辑并驱动同步循环。
 *
 * 引擎按批次原子应用（任一 op 越界则整批不落地），故 RPC 失败或应答行数与
 * UI 不符时，用「整侧重放」收敛而不是继续叠加可能错位的补丁。重放至多一次：
 * 仍不符说明 UI 行拆分与引擎口径不一致（如 CR-only 文件），此时清空队列、
 * 回落客户端重 diff 并标注，避免无限重放。
 */
export async function flushEdits(
  st: EditSyncState,
  ops: TextEditOp[],
  io: EditSyncIO,
): Promise<void> {
  st.queue.push(...ops);
  if (st.flushing) return; // 飞行中的循环会消费后续队列
  st.flushing = true;
  let resynced = false;
  let diverged = false;
  const replaySides = () => {
    const { left, right } = io.lines();
    st.queue = [fullSideReplayOp('left', left), fullSideReplayOp('right', right)];
  };
  try {
    while (st.queue.length > 0) {
      const edits = st.queue;
      st.queue = [];
      let res: TextEditResult | null;
      try {
        res = await io.edit(edits);
      } catch {
        if (resynced) {
          diverged = true;
          break;
        }
        resynced = true;
        replaySides();
        continue;
      }
      if (!res) {
        await io.fallback();
        return;
      }
      const { left, right } = io.lines();
      if (res.leftLineCount !== left.length || res.rightLineCount !== right.length) {
        if (resynced) {
          diverged = true;
          break;
        }
        resynced = true;
        replaySides();
        continue;
      }
      await io.adopt(res);
    }
  } finally {
    st.flushing = false;
  }
  if (diverged) {
    st.queue = [];
    io.diverged();
    await io.fallback();
  }
}
