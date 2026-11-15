import { beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../../ipc/engineProcess';
import type { TextEditOp, TextEditResult, TextSaveResult } from '../../protocol/types';
import { RpcError } from '../../protocol/client';

vi.mock('../../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return { ...actual, editText: vi.fn(), saveText: vi.fn() };
});

import { editText, saveText } from '../../ipc/engineProcess';
import { newEditDoc, type EditDoc } from './editModel';
import { describeSaveError, saveEditDoc, savedNotice } from './textEditSave';

const mockEdit = vi.mocked(editText);
const mockSave = vi.mocked(saveText);

/** 引擎行表（按真实语义应用 text/edit 的假实现） */
function installEngine(initial: string[]) {
  const table = [...initial];
  let rev = 0;
  mockEdit.mockImplementation(async (params) => {
    for (const op of params.edits as TextEditOp[]) {
      const end = op.end ?? table.length;
      const parts = op.text === '' ? [] : op.text.split('\n');
      if (parts.length > 1 && parts[parts.length - 1] === '') parts.pop();
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
  return table;
}

function docOf(lines: string[]): EditDoc {
  return newEditDoc({
    sessionId: 's',
    path: '/tmp/a.txt',
    name: 'a.txt',
    encoding: 'GB18030',
    lines,
  });
}

function saveResult(entry: Partial<TextSaveResult['saved'][number]> = {}): TextSaveResult {
  return {
    sessionId: 's',
    saved: [
      {
        side: 'left',
        path: '/tmp/a.txt',
        bytes: 12,
        lineCount: 2,
        ...entry,
      },
    ],
    rev: 1,
    leftModified: false,
    rightModified: false,
  } as TextSaveResult;
}

beforeEach(() => {
  mockEdit.mockReset();
  mockSave.mockReset();
});

describe('text-edit/textEditSave 落盘', () => {
  it('先收敛再落盘；成功后不再脏、回传目标与备份路径', async () => {
    const table = installEngine(['a', 'b']);
    mockSave.mockResolvedValue(saveResult({ backupPath: '/tmp/a.txt.bak' }));

    const doc = docOf(['a', 'b']);
    doc.lines = ['a', 'B'];
    const res = await saveEditDoc(doc, { backup: true });

    expect(res.ok).toBe(true);
    expect(res.path).toBe('/tmp/a.txt');
    expect(res.backupPath).toBe('/tmp/a.txt.bak');
    expect(res.retargeted).toBe(false);
    // 引擎先收到编辑增量（内容收敛），再收到保存
    expect(mockEdit).toHaveBeenCalledTimes(1);
    expect(table).toEqual(['a', 'B']);
    expect(mockSave).toHaveBeenCalledWith({
      sessionId: 's',
      side: 'left',
      backup: true,
      leftPath: undefined,
    });
    expect(doc.savedLines).toEqual(['a', 'B']);
    expect(savedNotice(res)).toContain('备份');
  });

  it('另存为：以引擎应答路径改指目标，retargeted=true', async () => {
    installEngine(['a']);
    mockSave.mockResolvedValue(saveResult({ path: '/tmp/b.txt' }));
    const doc = docOf(['a']);
    doc.lines = ['b'];
    const res = await saveEditDoc(doc, { path: '/tmp/b.txt' });
    expect(res.ok).toBe(true);
    expect(res.retargeted).toBe(true);
    expect(doc.path).toBe('/tmp/b.txt');
    expect(doc.name).toBe('b.txt');
  });

  it('保存失败（编码无法表示 413）：保留脏、不改目标、提示编码原因', async () => {
    installEngine(['a']);
    mockSave.mockRejectedValue(new RpcError(413, 'encoding', { message: 'encoding' } as never));
    const doc = docOf(['a']);
    doc.lines = ['中文'];
    const res = await saveEditDoc(doc);
    expect(res.ok).toBe(false);
    expect(res.message).toContain('编码');
    expect(res.path).toBe('/tmp/a.txt');
    expect(doc.savedLines).toEqual(['a']); // 仍脏
    expect(doc.path).toBe('/tmp/a.txt');
  });

  it('路径不存在（412）：提示路径原因', async () => {
    installEngine(['a']);
    mockSave.mockRejectedValue(new RpcError(412, 'nope', { message: 'nope' } as never));
    const res = await saveEditDoc(docOf(['a']));
    expect(res.ok).toBe(false);
    expect(res.message).toContain('路径');
  });

  it('引擎无 text/save（null）：不得当成已保存', async () => {
    installEngine(['a']);
    mockSave.mockResolvedValue(null);
    const doc = docOf(['a']);
    doc.lines = ['b'];
    const res = await saveEditDoc(doc);
    expect(res.ok).toBe(false);
    expect(res.message).toContain('不支持');
    expect(doc.savedLines).toEqual(['a']);
  });

  it('内容无法与引擎对齐：阻止保存（不得写陈旧内容）', async () => {
    mockEdit.mockResolvedValue({
      sessionId: 's',
      rev: 1,
      leftLineCount: 42, // 永远对不上 → 判分叉
      rightLineCount: 0,
      sectionCount: 0,
      summary: {},
      sections: [],
    } as unknown as TextEditResult);
    const doc = docOf(['a']);
    doc.lines = ['b'];
    const res = await saveEditDoc(doc);
    expect(res.ok).toBe(false);
    expect(res.message).toContain('对齐');
    expect(mockSave).not.toHaveBeenCalled();
    expect(doc.diverged).toBe(true);
  });

  it('引擎无 text/edit（null）：只读语义下也不得落盘', async () => {
    mockEdit.mockResolvedValue(null);
    const doc = docOf(['a']);
    doc.lines = ['b'];
    const res = await saveEditDoc(doc);
    expect(res.ok).toBe(false);
    expect(mockSave).not.toHaveBeenCalled();
  });

  it('describeSaveError：未知错误带出原始消息（不吞）', () => {
    expect(describeSaveError(new Error('boom'))).toContain('boom');
  });
});
