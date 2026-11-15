import { describe, expect, it, vi } from 'vitest';
import type { TextEditOp, TextEditResult } from '../../protocol/types';
import {
  computeLineEdit,
  deleteEditDoc,
  findDirtyEditDocByPath,
  docLinesOf,
  editOpFor,
  getEditDoc,
  isDocDirty,
  linesEqual,
  linesToEditText,
  newEditDoc,
  putEditDoc,
  replayOpFor,
  syncDocToEngine,
  type EditDoc,
} from './editModel';

/**
 * 行数组 ⇄ 引擎行表口径（D2）：换算错的后果是**保存丢内容**或反复触发重放，
 * 因此这里逐条钉住 `LineTable::from_text` 的语义（末尾换行不算新行、空串 = 零行）。
 */

/** 引擎侧解析口径（与 engine/ai-core `LineTable::from_text` 一致） */
function engineParse(text: string): string[] {
  if (text === '') return [];
  const parts = text.split('\n');
  if (parts.length > 1 && parts[parts.length - 1] === '') parts.pop();
  return parts;
}

describe('text-edit/editModel 行数组 ⇄ 文档文本', () => {
  it('docLinesOf：空文档 0 行；其余按 \\n 拆分（末尾空行保留）', () => {
    expect(docLinesOf('')).toEqual([]);
    expect(docLinesOf('a')).toEqual(['a']);
    expect(docLinesOf('a\nb')).toEqual(['a', 'b']);
    expect(docLinesOf('a\nb\n')).toEqual(['a', 'b', '']);
    expect(docLinesOf('\n')).toEqual(['', '']);
    // 空文本的歧义：默认 0 行，但引擎现状是"唯一空行"时保持不变
    expect(docLinesOf('', [''])).toEqual(['']);
    expect(docLinesOf('', ['a'])).toEqual([]);
    expect(docLinesOf('', [])).toEqual([]);
  });

  it('linesToEditText 与引擎解析互为逆运算（含空行/单空行）', () => {
    const cases: string[][] = [
      [],
      ['a'],
      ['a', 'b'],
      ['a', 'b', ''],
      [''],
      ['', ''],
      ['', 'x', ''],
      ['a', '', 'b'],
    ];
    for (const lines of cases) {
      const text = linesToEditText(lines);
      expect(engineParse(text), JSON.stringify(lines)).toEqual(lines);
      // 再走一遍文档口径（带 prev，消除空文本歧义）也必须一致
      expect(docLinesOf(lines.join('\n'), lines)).toEqual(lines);
    }
  });

  it('linesEqual：逐行比较（长度不同即不同）', () => {
    expect(linesEqual(['a'], ['a'])).toBe(true);
    expect(linesEqual(['a'], ['a', ''])).toBe(false);
    expect(linesEqual(['a'], ['b'])).toBe(false);
  });
});

describe('text-edit/editModel 最小行区间替换', () => {
  it('无变化返回 null', () => {
    expect(computeLineEdit(['a', 'b'], ['a', 'b'])).toBeNull();
    expect(computeLineEdit([], [])).toBeNull();
  });

  it('改一行：区间只覆盖该行', () => {
    expect(computeLineEdit(['a', 'b', 'c'], ['a', 'B', 'c'])).toEqual({
      start: 1,
      end: 2,
      text: 'B',
    });
  });

  it('中间插入一行：空区间 + 行文本', () => {
    expect(computeLineEdit(['a', 'c'], ['a', 'b', 'c'])).toEqual({
      start: 1,
      end: 1,
      text: 'b',
    });
  });

  it('末尾新增空行：text 必须带行尾符（否则引擎解析成 0 行）', () => {
    const span = computeLineEdit(['a', 'b'], ['a', 'b', '']);
    expect(span).toEqual({ start: 2, end: 2, text: '\n' });
    expect(engineParse(span!.text)).toEqual(['']);
  });

  it('删除全部内容：区间覆盖全表且 text 为空', () => {
    expect(computeLineEdit(['a', 'b'], [])).toEqual({ start: 0, end: 2, text: '' });
  });

  it('应用区间后的结果与目标完全一致（多种随机形态）', () => {
    const froms: string[][] = [
      [],
      ['a'],
      ['a', 'b'],
      ['a', 'b', 'c'],
      ['a', 'b', 'c', 'd'],
      ['', 'a', ''],
    ];
    const tos: string[][] = [
      [],
      ['a'],
      ['a', 'b'],
      ['x', 'a'],
      ['a', '', 'b'],
      ['a', 'b', 'c', 'd', 'e'],
      [''],
      ['', ''],
    ];
    for (const from of froms) {
      for (const to of tos) {
        const span = computeLineEdit(from, to);
        const applied =
          span === null
            ? [...from]
            : [...from.slice(0, span.start), ...engineParse(span.text), ...from.slice(span.end)];
        expect(applied, `${JSON.stringify(from)} -> ${JSON.stringify(to)}`).toEqual(to);
      }
    }
  });

  it('editOpFor / replayOpFor：单侧固定 left', () => {
    expect(editOpFor({ start: 1, end: 2, text: 'x' })).toEqual({
      side: 'left',
      start: 1,
      end: 2,
      text: 'x',
    });
    expect(replayOpFor(['a', ''])).toEqual({ side: 'left', start: 0, text: 'a\n\n' });
  });
});

/** 假引擎：持有行表，按真实语义应用 text/edit */
function fakeEngine(initial: string[]) {
  const table = [...initial];
  let rev = 0;
  const calls: TextEditOp[][] = [];
  const edit = vi.fn(async (edits: TextEditOp[]): Promise<TextEditResult | null> => {
    calls.push(edits);
    for (const op of edits) {
      const end = op.end ?? table.length;
      const parts = engineParse(op.text);
      table.splice(op.start, end - op.start, ...parts);
    }
    rev += 1;
    return {
      sessionId: 's',
      rev,
      leftLineCount: table.length,
      rightLineCount: 0,
      sectionCount: 0,
      summary: {},
      sections: [],
    } as unknown as TextEditResult;
  });
  return { table, edit, calls };
}

function docOf(lines: string[]): EditDoc {
  return newEditDoc({
    sessionId: 's',
    path: '/tmp/a.txt',
    name: 'a.txt',
    encoding: 'UTF-8',
    lines,
  });
}

describe('text-edit/editModel 引擎收敛', () => {
  it('内容一致时不发任何请求', async () => {
    const eng = fakeEngine(['a']);
    const doc = docOf(['a']);
    expect(await syncDocToEngine(doc, { edit: eng.edit })).toBe('ok');
    expect(eng.edit).not.toHaveBeenCalled();
  });

  it('本地编辑一次增量收敛，快照与行数同步', async () => {
    const eng = fakeEngine(['a', 'b']);
    const doc = docOf(['a', 'b']);
    doc.lines = ['a', 'B'];
    expect(await syncDocToEngine(doc, { edit: eng.edit })).toBe('ok');
    expect(eng.table).toEqual(['a', 'B']);
    expect(doc.engineSnapshot).toEqual(['a', 'B']);
    expect(doc.rev).toBe(1);
    expect(eng.calls.flat().length).toBe(1);
  });

  it('末尾新增空行：一次编辑即可收敛（不触发重放）', async () => {
    const eng = fakeEngine(['a', 'b']);
    const doc = docOf(['a', 'b']);
    doc.lines = ['a', 'b', ''];
    expect(await syncDocToEngine(doc, { edit: eng.edit })).toBe('ok');
    expect(eng.table).toEqual(['a', 'b', '']);
    expect(eng.calls.length).toBe(1);
  });

  it('行数不符时整侧重放一次即收敛', async () => {
    // 假引擎故意吞掉新增空行（模拟口径不一致的引擎），验证重放路径
    let first = true;
    const table = ['a'];
    const edit = vi.fn(async (edits: TextEditOp[]): Promise<TextEditResult | null> => {
      if (first) {
        first = false;
        // 忽略直接插入的空行（不落地）
        return {
          sessionId: 's',
          rev: 1,
          leftLineCount: table.length,
          rightLineCount: 0,
          sectionCount: 0,
          summary: {},
          sections: [],
        } as unknown as TextEditResult;
      }
      const op = edits[0];
      const parts = engineParse(op.text);
      table.splice(op.start, (op.end ?? table.length) - op.start, ...parts);
      return {
        sessionId: 's',
        rev: 2,
        leftLineCount: table.length,
        rightLineCount: 0,
        sectionCount: 0,
        summary: {},
        sections: [],
      } as unknown as TextEditResult;
    });
    const doc = docOf(['a']);
    doc.lines = ['a', ''];
    expect(await syncDocToEngine(doc, { edit })).toBe('ok');
    expect(edit).toHaveBeenCalledTimes(2);
    expect(table).toEqual(['a', '']);
    expect(doc.diverged).toBe(false);
  });

  it('重放后仍不符：判定分叉并停止（调用方据此阻止保存）', async () => {
    const edit = vi.fn(
      async (): Promise<TextEditResult | null> =>
        ({
          sessionId: 's',
          rev: 1,
          leftLineCount: 99,
          rightLineCount: 0,
          sectionCount: 0,
          summary: {},
          sections: [],
        }) as unknown as TextEditResult,
    );
    const doc = docOf(['a']);
    doc.lines = ['b'];
    expect(await syncDocToEngine(doc, { edit })).toBe('mismatch');
    expect(doc.diverged).toBe(true);
    expect(doc.engineSnapshot).toEqual(['a']); // 快照不前进：内容仍是引擎里那份
  });

  it('并发同步串行化：同一文档不会把同一区间应用两次', async () => {
    const eng = fakeEngine(['a']);
    const doc = docOf(['a']);
    doc.lines = ['X', 'a'];
    // 两个调用方（编辑防抖 + 保存/卸载冲刷）同时触发时，必须排队
    const [r1, r2] = await Promise.all([
      syncDocToEngine(doc, { edit: eng.edit }),
      syncDocToEngine(doc, { edit: eng.edit }),
    ]);
    expect(r1).toBe('ok');
    expect(r2).toBe('ok');
    expect(eng.table).toEqual(['X', 'a']); // 不是 ['X', 'X', 'a']
    expect(eng.calls.flat().length).toBe(1); // 第二次已一致，不再发请求
  });

  it('findDirtyEditDocByPath：只认领有未落盘编辑的同路径文档', () => {
    const clean = newEditDoc({
      sessionId: 'old-clean',
      path: '/p/a.txt',
      name: 'a.txt',
      encoding: 'UTF-8',
      lines: ['same'],
    });
    putEditDoc(clean);
    expect(findDirtyEditDocByPath('/p/a.txt', 'new')).toBeUndefined();

    const dirty = newEditDoc({
      sessionId: 'old-dirty',
      path: '/p/a.txt',
      name: 'a.txt',
      encoding: 'UTF-8',
      lines: ['edited'],
    });
    dirty.savedLines = ['same']; // 有未落盘编辑
    putEditDoc(dirty);
    expect(findDirtyEditDocByPath('/p/a.txt', 'new')).toBe(dirty);
    expect(findDirtyEditDocByPath('/p/a.txt', 'old-dirty')).toBeUndefined();
    deleteEditDoc('old-clean');
    deleteEditDoc('old-dirty');
  });

  it('引擎无 text/edit（null）：返回 unsupported 且不改快照', async () => {
    const edit = vi.fn(async () => null);
    const doc = docOf(['a']);
    doc.lines = ['b'];
    expect(await syncDocToEngine(doc, { edit })).toBe('unsupported');
    expect(doc.engineSnapshot).toEqual(['a']);
    expect(doc.diverged).toBe(false);
  });
});

describe('text-edit/editModel 脏判定与缓存', () => {
  it('isDocDirty 只与上次落盘内容比较', () => {
    const doc = docOf(['a']);
    expect(isDocDirty(doc)).toBe(false);
    doc.lines = ['b'];
    expect(isDocDirty(doc)).toBe(true);
    doc.savedLines = ['b'];
    expect(isDocDirty(doc)).toBe(false);
  });

  it('会话级缓存：put/get/delete', () => {
    const doc = docOf(['a']);
    putEditDoc(doc);
    expect(getEditDoc('s')).toBe(doc);
    deleteEditDoc('s');
    expect(getEditDoc('s')).toBeUndefined();
  });
});
