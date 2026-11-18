import type { DiffSummary, Section } from '../../protocol/types';
import type { EncodingId } from '../../lib/encodings';
import { emptyUndoState, type UndoState } from './undoStack';

/**
 * 文本比较会话级模型（视图层非响应式缓存，对齐 dirRowCache 的缓存形态）。
 *
 * 编辑内容 / 撤销栈 / 编码选择存活于本缓存（sessionId 键）：
 * 标签页切换不丢编辑（UNDO-001 会话级语义 + ENC-001 会话级编码），
 * 重新挂载时优先从缓存水合（避免引擎重开覆盖 UI 侧编辑）。
 * 引擎会话泄漏由标签关闭侧（P6 会话生命周期）统一收敛。
 */

export interface TextSessionModel {
  sessionId: string;
  leftPath: string;
  rightPath: string;
  leftName: string;
  rightName: string;
  /** 当前（可能已被编辑）单侧内容行 */
  leftLines: string[];
  rightLines: string[];
  sections: Section[];
  summary: DiffSummary;
  undo: UndoState;
  encoding: EncodingId;
  /** 引擎回传的生效编码（R3-15）；null = 未知（复用已存在会话时无 open 应答） */
  detectedEncoding: { left: string | null; right: string | null };
  /** 逐侧未落盘编辑标记（R3-04：保存按侧清除；父目录刷新与关闭守卫据此判定） */
  modifiedSides: { left: boolean; right: boolean };
  /** 行内差异 DMP 语义清理开关（引擎 inlineDiff RPC 参数映射） */
  dmpCleanup: boolean;
  /** 折叠状态（会话级：已折叠等值段 key 集合，标签切换恢复） */
  collapsedKeys: number[];
  /** 折叠上下文行数（会话级） */
  contextRows: number;
}

/** 任一侧有未落盘编辑 */
export function isTextModified(model: TextSessionModel): boolean {
  return model.modifiedSides.left || model.modifiedSides.right;
}

const CACHE_LIMIT = 20;
const cache = new Map<string, TextSessionModel>();

export function getTextSession(sessionId: string): TextSessionModel | undefined {
  return cache.get(sessionId);
}

export function putTextSession(model: TextSessionModel): void {
  cache.delete(model.sessionId);
  cache.set(model.sessionId, model);
  if (cache.size > CACHE_LIMIT) {
    // UI-03：淘汰跳过脏项——脏标记是关闭守卫与崩溃认领的唯一依据，
    // 淘汰即静默丢编辑。脏项数受打开标签数天然约束，全脏时缓存暂超限。
    for (const key of cache.keys()) {
      const m = cache.get(key);
      if (m && !isTextModified(m)) {
        cache.delete(key);
        break;
      }
    }
  }
}

export function deleteTextSession(sessionId: string): void {
  cache.delete(sessionId);
}

/**
 * 按落盘路径对找回**有未落盘编辑**的旧会话模型（崩溃重放认领，UI-01）。
 *
 * 引擎崩溃重放给会话换新句柄，视图按新句柄重开同一文件对（引擎内容回到
 * 文件态），旧模型里的未保存编辑仍在缓存——按路径认领回来再整侧重放收敛。
 * 只认领 isLive 判为"已死"的会话：同文件对可能另有活标签在编辑，不得抢它。
 * isLive 缺省恒真 = 不传判定就不认领（安全缺省）。
 */
export function findDirtyTextSessionByPaths(
  leftPath: string,
  rightPath: string,
  excludeSessionId?: string,
  isLive: (sessionId: string) => boolean = () => true,
): TextSessionModel | undefined {
  for (const model of cache.values()) {
    if (model.sessionId === excludeSessionId) continue;
    if (isLive(model.sessionId)) continue;
    if (
      model.leftPath === leftPath &&
      model.rightPath === rightPath &&
      isTextModified(model)
    ) {
      return model;
    }
  }
  return undefined;
}

/** 由引擎打开结果构建初始模型（undo 清空、编码 auto、未修改） */
export function newTextSessionModel(init: {
  sessionId: string;
  leftPath: string;
  rightPath: string;
  leftName: string;
  rightName: string;
  leftLines: string[];
  rightLines: string[];
  sections: Section[];
  summary: DiffSummary;
  detectedEncoding?: { left: string | null; right: string | null };
}): TextSessionModel {
  return {
    ...init,
    undo: emptyUndoState(),
    encoding: 'auto',
    detectedEncoding: init.detectedEncoding ?? { left: null, right: null },
    modifiedSides: { left: false, right: false },
    dmpCleanup: false,
    collapsedKeys: [],
    contextRows: 3,
  };
}
