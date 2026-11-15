import { describe, it, expect } from 'vitest';
import { formatSize } from './env';

describe('lib/env formatSize 单位换算', () => {
  it('不足 1KB 按字节显示', () => {
    expect(formatSize(undefined)).toBe('');
    expect(formatSize(0)).toBe('0 B');
    expect(formatSize(1023)).toBe('1023 B');
  });

  it('KB 段：4293 B = 4.2 KB（不得标成 MB）', () => {
    expect(formatSize(1024)).toBe('1.0 KB');
    expect(formatSize(4293)).toBe('4.2 KB');
    expect(formatSize(12348)).toBe('12.1 KB');
    expect(formatSize(156796)).toBe('153 KB');
  });

  it('MB/GB/TB 段逐级进位', () => {
    expect(formatSize(1024 * 1024)).toBe('1.0 MB');
    expect(formatSize(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(formatSize(153 * 1024 * 1024)).toBe('153 MB');
    expect(formatSize(1024 ** 3)).toBe('1.0 GB');
    expect(formatSize(1024 ** 4)).toBe('1.0 TB');
    expect(formatSize(3 * 1024 ** 4)).toBe('3.0 TB');
  });
});
