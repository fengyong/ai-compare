import { describe, expect, it } from 'vitest';
import { LANG_LABELS, detectLangId } from './langDetect';

describe('langDetect：语法高亮语言探测（FR-TXT-013/014 简化版）', () => {
  it('常用集扩展名映射：js/ts/json/md/python/rust', () => {
    expect(detectLangId('main.js')).toBe('javascript');
    expect(detectLangId('app.mjs')).toBe('javascript');
    expect(detectLangId('index.ts')).toBe('typescript');
    expect(detectLangId('comp.tsx')).toBe('typescript');
    expect(detectLangId('package.json')).toBe('json');
    expect(detectLangId('README.md')).toBe('markdown');
    expect(detectLangId('main.py')).toBe('python');
    expect(detectLangId('lib.rs')).toBe('rust');
  });

  it('大小写不敏感（README.MD）', () => {
    expect(detectLangId('README.MD')).toBe('markdown');
    expect(detectLangId('Data.JSON')).toBe('json');
  });

  it('无扩展名 / 未知扩展名 → null（回退纯文本）', () => {
    expect(detectLangId('Makefile')).toBeNull();
    expect(detectLangId('data.xyz')).toBeNull();
    expect(detectLangId('')).toBeNull();
    expect(detectLangId('trailing.')).toBeNull();
  });

  it('语言显示名全集覆盖', () => {
    expect(Object.keys(LANG_LABELS).sort()).toEqual(
      ['javascript', 'json', 'markdown', 'python', 'rust', 'typescript'].sort(),
    );
  });
});
