// @vitest-environment jsdom
/**
 * Hex 跳转/分页并发代际守卫回归（UI-04）。
 *
 * hexBytes mock 成可控延迟应答：跳转与滚动交错时，旧代际的迟到应答
 * （含错误）必须丢弃——列表地址严格单调等于请求序，不得拼接错页。
 */
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as MockedModule from '../../ipc/engineProcess';
import type { EngineCapabilities } from '../../protocol/types';
import type { Tab } from '../../shell/shellState';

vi.mock('../../ipc/engineProcess', async (importOriginal) => {
  const actual = await importOriginal<typeof MockedModule>();
  return {
    ...actual,
    hexBytes: vi.fn(),
    openHexIdempotent: vi.fn(),
    getEngineCapabilities: vi.fn(),
    trackSession: vi.fn(),
  };
});

import { getEngineCapabilities, hexBytes, openHexIdempotent } from '../../ipc/engineProcess';
import { HexCompareView } from './HexCompareView';

type HexLine = { addr: number; hex: string; ascii: string; status: string };
type Pending = {
  side: string;
  offset: number;
  resolve: (r: { lines: HexLine[]; done: boolean }) => void;
  reject: (e: Error) => void;
};

let pending: Pending[] = [];

function lines(start: number, n: number): HexLine[] {
  return Array.from({ length: n }, (_, i) => ({
    addr: start + i * 16,
    hex: '00 '.repeat(16).trim(),
    ascii: '.'.repeat(16),
    status: '='.repeat(16),
  }));
}

function take(side: string, offset: number): Pending {
  const i = pending.findIndex((p) => p.side === side && p.offset === offset);
  expect(i, `应有在途请求 ${side}@${offset}，实际: ${JSON.stringify(pending.map((p) => [p.side, p.offset]))}`).toBeGreaterThanOrEqual(0);
  return pending.splice(i, 1)[0];
}

function addrs(container: HTMLElement, side: string): number[] {
  return [...container.querySelectorAll(`[data-side="${side}"] .hx-addr`)].map((td) =>
    parseInt(td.textContent ?? '', 16),
  );
}

const TAB = {
  id: 't-hex',
  kind: 'hex',
  title: '十六进制比较：a ⇄ b',
  hex: { left: '/tmp/a.bin', right: '/tmp/b.bin', sessionId: 's-hex' },
} as Tab & { kind: 'hex' };

function jump(container: HTMLElement, hexAddr: string): void {
  const input = container.querySelector('.aic-hex-jump') as HTMLInputElement;
  fireEvent.change(input, { target: { value: hexAddr } });
  fireEvent.keyDown(input, { key: 'Enter' });
}

beforeEach(() => {
  pending = [];
  vi.mocked(hexBytes).mockImplementation(
    (p) =>
      new Promise((resolve, reject) => {
        pending.push({ side: p.side, offset: p.offset, resolve, reject });
      }),
  );
  vi.mocked(openHexIdempotent).mockResolvedValue({
    sessionId: 's-hex',
    leftSize: 1 << 20,
    rightSize: 1 << 20,
    blockCount: 0,
    summary: { sameBytes: 0, changedBytes: 0, insertedBytes: 0, deletedBytes: 0 },
  });
  vi.mocked(getEngineCapabilities).mockReturnValue({ hexCompare: true } as EngineCapabilities);
  // jsdom 无 scrollIntoView
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function renderReady(): Promise<HTMLElement> {
  const { container } = render(<HexCompareView tab={TAB} />);
  // open 就绪后左右两 pane 各发首页请求
  await waitFor(() => expect(pending.length).toBe(2));
  return container;
}

describe('Hex 代际守卫（UI-04）', () => {
  it('在途分页迟到应答不得拼进跳转后的新页', async () => {
    const container = await renderReady();
    jump(container, '0x1000');
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 4096)).toBe(true));
    // 新页先落地
    take('left', 4096).resolve({ lines: lines(4096, 4), done: false });
    await waitFor(() => expect(addrs(container, 'left')).toEqual([0x1000, 0x1010, 0x1020, 0x1030]));
    // 旧首页迟到 → 必须丢弃
    take('left', 0).resolve({ lines: lines(0, 4), done: false });
    await new Promise((r) => setTimeout(r, 20));
    expect(addrs(container, 'left')).toEqual([0x1000, 0x1010, 0x1020, 0x1030]);
  });

  it('连续两次跳转：先发后至的旧跳转应答不得覆盖新页', async () => {
    const container = await renderReady();
    jump(container, '0x1000');
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 4096)).toBe(true));
    jump(container, '0x2000');
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 8192)).toBe(true));
    take('left', 8192).resolve({ lines: lines(8192, 3), done: false });
    await waitFor(() => expect(addrs(container, 'left')).toEqual([0x2000, 0x2010, 0x2020]));
    take('left', 4096).resolve({ lines: lines(4096, 3), done: false });
    await new Promise((r) => setTimeout(r, 20));
    expect(addrs(container, 'left')).toEqual([0x2000, 0x2010, 0x2020]);
  });

  it('旧代际的迟到错误不得污染新页', async () => {
    const container = await renderReady();
    jump(container, '0x1000');
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 4096)).toBe(true));
    jump(container, '0x2000');
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 8192)).toBe(true));
    take('left', 8192).resolve({ lines: lines(8192, 2), done: false });
    await waitFor(() => expect(addrs(container, 'left')).toEqual([0x2000, 0x2010]));
    // 迟到的是旧跳转请求（走跳转 catch 路径）
    take('left', 4096).reject(new Error('stale failure'));
    await new Promise((r) => setTimeout(r, 20));
    expect(addrs(container, 'left')).toEqual([0x2000, 0x2010]);
    expect(container.querySelector('[data-side="left"] .hx-loading')?.textContent ?? '').not.toContain('stale');
  });

  it('旧代际分页的迟到错误同样不得污染新页', async () => {
    const container = await renderReady();
    jump(container, '0x1000');
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 4096)).toBe(true));
    take('left', 4096).resolve({ lines: lines(4096, 2), done: false });
    await waitFor(() => expect(addrs(container, 'left')).toEqual([0x1000, 0x1010]));
    // 迟到的是旧分页请求（走 loadNext catch 路径）
    take('left', 0).reject(new Error('stale page failure'));
    await new Promise((r) => setTimeout(r, 20));
    expect(addrs(container, 'left')).toEqual([0x1000, 0x1010]);
    expect(container.querySelector('[data-side="left"] .hx-loading')?.textContent ?? '').not.toContain('stale');
  });

  it('正常滚动分页不受影响：追加页地址严格连续', async () => {
    const container = await renderReady();
    take('left', 0).resolve({ lines: lines(0, 256), done: false });
    await waitFor(() => expect(addrs(container, 'left').length).toBe(256));
    fireEvent.scroll(container.querySelector('[data-side="left"]')!);
    await waitFor(() => expect(pending.some((p) => p.side === 'left' && p.offset === 4096)).toBe(true));
    take('left', 4096).resolve({ lines: lines(4096, 2), done: true });
    await waitFor(() => expect(addrs(container, 'left').length).toBe(258));
    const a = addrs(container, 'left');
    expect(a[0]).toBe(0);
    expect(a[256]).toBe(4096);
    expect(a[257]).toBe(4112);
    for (let i = 1; i < a.length; i++) expect(a[i]).toBe(a[i - 1] + 16);
  });
});