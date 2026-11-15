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
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
}

export function deleteTextSession(sessionId: string): void {
  cache.delete(sessionId);
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
