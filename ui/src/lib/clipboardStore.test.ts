import { describe, expect, it } from 'vitest';
import {
  DEFAULT_CLIP_LIMITS,
  appendClip,
  clipByteLength,
  clipLabel,
  clipTotalBytes,
  latestTwo,
  parseClipSpec,
  ClipboardStore,
  type ClipboardEntry,
} from './clipboardStore';

const e = (id: number, text: string): ClipboardEntry => ({ id, text, ts: id * 1000 });

describe('剪贴板历史纯函数（FR-CLIP-003 限制语义）', () => {
  it('空文本 / 低于最小长度 / 与上一条相同 → 不新增', () => {
    const h = [e(1, 'first')];
    expect(appendClip(h, '', 2, 2)).toEqual(h);
    expect(appendClip(h, 'x', 2, 2, { ...DEFAULT_CLIP_LIMITS, minLength: 3 })).toEqual(h);
    expect(appendClip(h, 'first', 2, 2)).toEqual(h);
    expect(appendClip(h, 'second', 2, 2)).toHaveLength(2);
  });

  it('条数上限裁剪保留最新 N 条', () => {
    let h: ClipboardEntry[] = [];
    for (let i = 1; i <= 8; i++)
      h = appendClip(h, `line-${i}`, i, i, { ...DEFAULT_CLIP_LIMITS, maxEntries: 3 });
    expect(h.map((x) => x.text)).toEqual(['line-6', 'line-7', 'line-8']);
  });

  it('内存上限裁剪但至少保留最近两条（FR-CLIP-003）', () => {
    const big = 'x'.repeat(100);
    let h: ClipboardEntry[] = [];
    for (let i = 1; i <= 6; i++) {
      h = appendClip(h, `${big}${i}`, i, i, {
        ...DEFAULT_CLIP_LIMITS,
        maxBytes: 250,
        maxEntries: 20,
      });
    }
    expect(h.length).toBeGreaterThanOrEqual(2);
    expect(h.length).toBeLessThan(6);
    expect(h[h.length - 1].text).toBe(`${big}6`);
    expect(clipTotalBytes(h.slice(0, -2))).toBeLessThanOrEqual(250);
  });

  it('最近两条按较旧在左、最新在右', () => {
    expect(latestTwo([e(1, 'a')])).toBeNull();
    expect(latestTwo([e(1, 'a'), e(2, 'b'), e(3, 'c')])?.map((x) => x.text)).toEqual(['b', 'c']);
  });

  it('clipboard:// 规格解析与普通路径区分', () => {
    expect(parseClipSpec('clipboard://12')).toBe(12);
    expect(parseClipSpec('clipboard://0')).toBe(0);
    expect(parseClipSpec('/tmp/a.txt')).toBeNull();
    expect(parseClipSpec('clipboard://abc')).toBeNull();
    expect(parseClipSpec('zip://x/clipboard://3')).toBeNull();
  });

  it('标签取首个非空行并截断；字节数按 UTF-8', () => {
    expect(clipLabel(e(1, '\n\n  hello world  \nsecond'))).toBe('hello world');
    expect(clipLabel(e(2, 'x'.repeat(60)), 10)).toBe('xxxxxxxxxx…');
    expect(clipLabel(e(3, '\n  \n'))).toBe('（空白）');
    expect(clipByteLength('你好')).toBe(6);
  });
});

describe('ClipboardStore 单例行为', () => {
  it('add/registerText/getText/remove/clear 与订阅', () => {
    const s = new ClipboardStore('unit-clip-store');
    let hits = 0;
    const off = s.subscribe(() => hits++);
    const first = s.add('alpha');
    expect(first?.text).toBe('alpha');
    expect(s.getText(first!.id)).toBe('alpha');
    const spec = s.registerText('beta');
    expect(parseClipSpec(spec)).toBe(s.list()[s.list().length - 1].id);
    expect(hits).toBe(2);
    s.remove(first!.id);
    expect(s.getText(first!.id)).toBeUndefined();
    s.clear();
    expect(s.list()).toHaveLength(0);
    off();
  });

  it('历史已满时 add 仍返回新增条目（裁剪不会误判为过滤）', () => {
    const s = new ClipboardStore('unit-clip-full');
    s.setLimits({ maxEntries: 3 });
    s.add('a');
    s.add('b');
    s.add('c');
    const fourth = s.add('d');
    expect(fourth?.text).toBe('d');
    expect(s.list().map((e) => e.text)).toEqual(['b', 'c', 'd']);
  });

  it('registerText 边界：空历史/空文本/相邻重复都返回可用规格', async () => {
    const s = new ClipboardStore('unit-clip-register');
    const p1 = s.registerText('');
    expect(parseClipSpec(p1)).not.toBeNull();
    expect(s.getText(parseClipSpec(p1)!)).toBe('');
    const p2 = s.registerText('hello');
    expect(s.getText(parseClipSpec(p2)!)).toBe('hello');
    expect(s.registerText('hello')).toBe(p2); // 相邻重复复用同一编号
    await expect(s.materialize(p2)).resolves.toBe(p2);
  });

  it('非 Tauri 环境 materialize 保留规格；未知 id 抛错', async () => {
    const s = new ClipboardStore('unit-clip-store-2');
    const spec = s.registerText('hello');
    await expect(s.materialize(spec)).resolves.toBe(spec);
    await expect(s.materialize('/tmp/ordinary.txt')).resolves.toBe('/tmp/ordinary.txt');
    await expect(s.materialize('clipboard://9999')).rejects.toThrow(/不存在/);
  });
});
