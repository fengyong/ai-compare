import type { DiffSummary, Section, TextOpenResult } from '../../protocol/types';
import * as engine from '../../ipc/engineProcess';

/**
 * 文本会话加载编排（text/open → text/sections 分页拉全 → 行内容加载）。
 *
 * P0 说明：CM6 文档采用"一次性构建完整对齐文档"的形态（CM6 自身只渲染可视区，
 * 滚动性能由编辑器虚拟化保障；PB-05 正式线在 P5 达标）。
 * - sections 经 text/sections {offset, count} 窗口化分页拉全（生成协议口径）；
 * - 行内容：text/lines（演示引擎扩展）→ 真实引擎未开放时走 Tauri 兜底通道
 *   读本地文件（engineProcess.loadSideLines，P1 引擎开放 text/lines 后收敛）。
 */

export interface LoadedTextSession {
  sessionId: string;
  leftLineCount: number;
  rightLineCount: number;
  sections: Section[];
  summary: DiffSummary;
  leftLines: string[];
  rightLines: string[];
}

const SECTIONS_CHUNK = 4000;

async function loadAllSections(sessionId: string): Promise<Section[]> {
  const all: Section[] = [];
  let offset = 0;
  let guard = 0;
  while (guard++ < 10_000) {
    const res = await engine.getTextSections({ sessionId, offset, count: SECTIONS_CHUNK });
    all.push(...res.sections);
    offset += res.sections.length;
    if (!res.hasMore || res.sections.length === 0) break;
  }
  return all;
}

/** 打开结果（含行数与统计）→ 全量加载 sections 与两侧行内容 */
export async function loadTextSession(
  opened: TextOpenResult,
  leftPath: string,
  rightPath: string
): Promise<LoadedTextSession> {
  const sessionId = opened.sessionId;
  const [sections, leftLines, rightLines] = await Promise.all([
    loadAllSections(sessionId),
    engine.loadSideLines(sessionId, 'left', opened.leftLineCount, leftPath),
    engine.loadSideLines(sessionId, 'right', opened.rightLineCount, rightPath),
  ]);
  return {
    sessionId,
    leftLineCount: opened.leftLineCount,
    rightLineCount: opened.rightLineCount,
    sections,
    summary: opened.summary,
    leftLines,
    rightLines,
  };
}
