import type { DiffSummary, Section, TextOpenResult, TextSideMeta } from '../../protocol/types';
import * as engine from '../../ipc/engineProcess';

/**
 * 文本会话加载编排（text/open → text/sections → text/lines 分页拉全）。
 *
 * P0 说明：CM6 文档采用"一次性构建完整对齐文档"的形态（CM6 自身只渲染可视区，
 * 滚动性能由编辑器虚拟化保障；PB-05 正式线在 P5 达标）。行内容经 text/lines
 * 分页拉取，协议形态与大文件分页语义一致（docs/03 §2.3.2 大内容），
 * 后续演进为增量追加时协议不变。
 */

export interface LoadedTextSession {
  sessionId: string;
  left: TextSideMeta;
  right: TextSideMeta;
  sections: Section[];
  summary: DiffSummary;
  leftLines: string[];
  rightLines: string[];
}

const LINES_CHUNK = 4000;

async function loadSideLines(
  sessionId: string,
  side: 'left' | 'right',
  lineCount: number
): Promise<string[]> {
  const lines: string[] = [];
  let start = 0;
  // 防御性上限：协议异常（done 永不置位）时避免死循环
  let guard = 0;
  while (start < lineCount && guard++ < 10_000) {
    const res = await engine.getTextLines({ sessionId, side, start, count: LINES_CHUNK });
    lines.push(...res.lines);
    if (res.done || res.lines.length === 0) break;
    start += res.lines.length;
  }
  return lines;
}

/** 打开结果（含行数元数据）→ 全量加载 sections 与两侧行内容 */
export async function loadTextSession(opened: TextOpenResult): Promise<LoadedTextSession> {
  const sessionId = opened.sessionId;
  const [sectionsRes, leftLines, rightLines] = await Promise.all([
    engine.getTextSections({ sessionId }),
    loadSideLines(sessionId, 'left', opened.left.lineCount),
    loadSideLines(sessionId, 'right', opened.right.lineCount),
  ]);
  return {
    sessionId,
    left: opened.left,
    right: opened.right,
    sections: sectionsRes.sections,
    summary: sectionsRes.summary,
    leftLines,
    rightLines,
  };
}
