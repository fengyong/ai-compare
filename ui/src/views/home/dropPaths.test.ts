import { describe, it, expect } from 'vitest';
import { extractPathsFromSource, fileUriToPath, pickTwoPaths } from './dropPaths';

describe('views/home/dropPaths 拖放路径解析', () => {
  it('fileUriToPath：解码 file:// URI（空格/中文/盘符）', () => {
    expect(fileUriToPath('file:///tmp/My%20Folder')).toBe('/tmp/My Folder');
    expect(fileUriToPath('file:///tmp/%E4%B8%AD%E6%96%87')).toBe('/tmp/中文');
    expect(fileUriToPath('file:///C:/Users/a')).toBe('C:/Users/a');
    expect(fileUriToPath('http://x/a')).toBeNull();
  });

  it('text/plain 多行 → 两条路径', () => {
    const out = extractPathsFromSource({ text: '/tmp/L\n/tmp/R\n' });
    expect(out).toEqual(['/tmp/L', '/tmp/R']);
  });

  it('text/plain 中的 file:// 行解码', () => {
    const out = extractPathsFromSource({ text: 'file:///a/L\nfile:///b/R' });
    expect(out).toEqual(['/a/L', '/b/R']);
  });

  it('uri-list 优先于 text；#注释行忽略', () => {
    const out = extractPathsFromSource({
      uriList: '# comment\nfile:///u/L\nfile:///u/R',
      text: '/t/L\n/t/R',
    });
    expect(out).toEqual(['/u/L', '/u/R']);
  });

  it('无 text/uri 时回退 entryPaths（浏览器 webkitGetAsEntry 相对路径）', () => {
    const out = extractPathsFromSource({ entryPaths: ['L-folder', 'R-folder'] });
    expect(out).toEqual(['L-folder', 'R-folder']);
  });

  it('去重去空', () => {
    const out = extractPathsFromSource({ text: '/a\n\n/a\n/b' });
    expect(out).toEqual(['/a', '/b']);
  });

  it('pickTwoPaths：两个 → 左右；一个 → 左侧+提示；空 → 全空+提示', () => {
    expect(pickTwoPaths(['/L', '/R'])).toEqual({ left: '/L', right: '/R' });
    const one = pickTwoPaths(['/L']);
    expect(one.left).toBe('/L');
    expect(one.right).toBeNull();
    expect(one.warning).toBeTruthy();
    const none = pickTwoPaths([]);
    expect(none.left).toBeNull();
    expect(none.warning).toBeTruthy();
    const many = pickTwoPaths(['/1', '/2', '/3']);
    expect(many).toEqual({ left: '/1', right: '/2' });
  });
});
