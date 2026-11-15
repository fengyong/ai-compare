import { describe, expect, it } from 'vitest';
import {
  DEMO_LEFT_FILES,
  DEMO_RIGHT_FILES,
  decodeAsset,
  demoFilePlan,
  demoMerge3Paths,
  demoRoots,
  seedDemoDirs,
  type DemoFileSpec,
} from './demoSeed';
import { DEMO_ASSETS } from './demoAssets';
import { MemoryFileStore } from '../session/storageAdapter';

/**
 * 全格式演示数据（真实引擎模式）：
 * 目录 diff 需覆盖四种状态（相同/不同/仅左/仅右）+ 改名候选，
 * 且文件清单需覆盖全部 8 类视图（文本/表格/图片/hex/zip/编码/合并/大文件），
 * 否则用户点「填充示例路径」后无法体验完整功能。
 */

const byRel = (files: DemoFileSpec[], rel: string) => files.find((f) => f.rel === rel);
const text = (files: DemoFileSpec[], rel: string) => byRel(files, rel)?.contents;

describe('四态与改名', () => {
  it('README 与 util.rs 两侧内容一致（相同）', () => {
    expect(text(DEMO_LEFT_FILES, 'README.md')).toBe(text(DEMO_RIGHT_FILES, 'README.md'));
    expect(text(DEMO_LEFT_FILES, 'src/util.rs')).toBe(text(DEMO_RIGHT_FILES, 'src/util.rs'));
  });

  it('main.rs 与 config.json 两侧内容不同（不同）', () => {
    expect(text(DEMO_LEFT_FILES, 'src/main.rs')).not.toBe(text(DEMO_RIGHT_FILES, 'src/main.rs'));
    expect(text(DEMO_LEFT_FILES, 'config.json')).not.toBe(text(DEMO_RIGHT_FILES, 'config.json'));
  });

  it('存在仅左 / 仅右条目（孤立与新增）', () => {
    expect(byRel(DEMO_LEFT_FILES, 'docs/only-left.md')).toBeTruthy();
    expect(byRel(DEMO_RIGHT_FILES, 'docs/only-left.md')).toBeUndefined();
    expect(byRel(DEMO_RIGHT_FILES, 'src/new.rs')).toBeTruthy();
    expect(byRel(DEMO_LEFT_FILES, 'src/new.rs')).toBeUndefined();
  });

  it('改名候选：old-name(左) 与 new-name(右) 内容相同', () => {
    expect(text(DEMO_LEFT_FILES, 'docs/old-name.md')).toBe(
      text(DEMO_RIGHT_FILES, 'docs/new-name.md'),
    );
    expect(byRel(DEMO_RIGHT_FILES, 'docs/old-name.md')).toBeUndefined();
    expect(byRel(DEMO_LEFT_FILES, 'docs/new-name.md')).toBeUndefined();
  });
});

describe('全格式覆盖', () => {
  const rels = (files: DemoFileSpec[]) => files.map((f) => f.rel);

  it('表格视图：csv/tsv/tab 齐备', () => {
    for (const r of ['data/users.csv', 'data/users.tsv', 'data/products.tab']) {
      expect(rels(DEMO_LEFT_FILES)).toContain(r);
      expect(rels(DEMO_RIGHT_FILES)).toContain(r);
    }
  });

  it('图片视图：png 内容差异/尺寸差异/相同/仅左', () => {
    expect(byRel(DEMO_LEFT_FILES, 'images/logo.png')?.asset).toBe('pngLogoL');
    expect(byRel(DEMO_RIGHT_FILES, 'images/logo.png')?.asset).toBe('pngLogoR');
    expect(byRel(DEMO_LEFT_FILES, 'images/banner.png')?.asset).toBe('pngBannerL');
    expect(byRel(DEMO_RIGHT_FILES, 'images/banner.png')?.asset).toBe('pngBannerR');
    expect(byRel(DEMO_LEFT_FILES, 'images/icon.png')?.asset).toBe('pngIcon');
    expect(byRel(DEMO_RIGHT_FILES, 'images/icon.png')?.asset).toBe('pngIcon');
    expect(byRel(DEMO_LEFT_FILES, 'images/only-left.png')?.asset).toBe('pngOnlyLeft');
    expect(byRel(DEMO_RIGHT_FILES, 'images/only-left.png')).toBeUndefined();
  });

  it('hex 视图：dat/bin/o 齐备且含仅左/仅右', () => {
    expect(byRel(DEMO_LEFT_FILES, 'binary/blob.dat')?.asset).toBe('blobL');
    expect(byRel(DEMO_RIGHT_FILES, 'binary/blob.dat')?.asset).toBe('blobR');
    expect(byRel(DEMO_LEFT_FILES, 'binary/firmware.bin')).toBeTruthy();
    expect(byRel(DEMO_RIGHT_FILES, 'binary/firmware.bin')).toBeUndefined();
    expect(byRel(DEMO_RIGHT_FILES, 'binary/lib.o')).toBeTruthy();
    expect(byRel(DEMO_LEFT_FILES, 'binary/lib.o')).toBeUndefined();
  });

  it('zip 视图：两侧归档资产不同', () => {
    expect(byRel(DEMO_LEFT_FILES, 'archives/data.zip')?.asset).toBe('zipDataL');
    expect(byRel(DEMO_RIGHT_FILES, 'archives/data.zip')?.asset).toBe('zipDataR');
  });

  it('编码：GB18030 与 UTF-16 资产两侧不同', () => {
    expect(byRel(DEMO_LEFT_FILES, 'encoding/gb18030.txt')?.asset).toBe('gbL');
    expect(byRel(DEMO_RIGHT_FILES, 'encoding/gb18030.txt')?.asset).toBe('gbR');
    expect(byRel(DEMO_LEFT_FILES, 'encoding/utf16.txt')?.asset).toBe('u16L');
    expect(byRel(DEMO_RIGHT_FILES, 'encoding/utf16.txt')?.asset).toBe('u16R');
  });

  it('行尾/BOM/Unicode 文本变体', () => {
    expect(text(DEMO_LEFT_FILES, 'text/crlf.txt')).toContain('\r\n');
    expect(text(DEMO_RIGHT_FILES, 'text/crlf.txt')).not.toContain('\r\n');
    expect(text(DEMO_LEFT_FILES, 'text/bom.txt')?.startsWith('﻿')).toBe(true);
    expect(text(DEMO_RIGHT_FILES, 'text/bom.txt')?.startsWith('﻿')).toBe(false);
    expect(text(DEMO_LEFT_FILES, 'text/unicode.txt')).toBe(
      text(DEMO_RIGHT_FILES, 'text/unicode.txt'),
    );
  });

  it('隐藏文件与隐藏目录', () => {
    expect(byRel(DEMO_LEFT_FILES, '.env')).toBeTruthy();
    expect(rels(DEMO_LEFT_FILES)).toContain('.gitignore');
    expect(rels(DEMO_RIGHT_FILES)).toContain('.hidden/secret.txt');
  });

  it('嵌套深层与空目录标记', () => {
    expect(rels(DEMO_LEFT_FILES)).toContain('nested/deep/a/b/c.txt');
    expect(byRel(DEMO_LEFT_FILES, 'nested/empty-left/.gitkeep')).toBeTruthy();
    expect(byRel(DEMO_RIGHT_FILES, 'nested/empty-left/.gitkeep')).toBeUndefined();
    expect(byRel(DEMO_RIGHT_FILES, 'nested/empty-right/.gitkeep')).toBeTruthy();
  });
});

describe('三向合并与大文件', () => {
  it('merge3 三文件互不相同：name 冲突、mode 仅右、retry 仅左', () => {
    const base = text(DEMO_LEFT_FILES, 'merge3/base.md')!;
    const left = text(DEMO_LEFT_FILES, 'merge3/left.md')!;
    const right = text(DEMO_LEFT_FILES, 'merge3/right.md')!;
    expect(base).not.toBe(left);
    expect(base).not.toBe(right);
    expect(left).toContain('name = demo-left');
    expect(right).toContain('name = demo-right');
    expect(left).toContain('retry = 5');
    expect(right).toContain('mode = safe');
    expect(base).toContain('retry = 3');
    expect(base).toContain('mode = fast');
  });

  it('demoMerge3Paths 指向左根 merge3/ 下三文件', () => {
    const p = demoMerge3Paths('/tmp/base');
    expect(p.base).toBe('/tmp/base/demo/project-v1/merge3/base.md');
    expect(p.left).toBe('/tmp/base/demo/project-v1/merge3/left.md');
    expect(p.right).toBe('/tmp/base/demo/project-v1/merge3/right.md');
  });

  it('大文件 4000 行，右侧恰 3 处 [CHANGED]', () => {
    const l = text(DEMO_LEFT_FILES, 'large/big.txt')!;
    const r = text(DEMO_RIGHT_FILES, 'large/big.txt')!;
    expect(l.split('\n').length - 1).toBe(4000);
    expect(r.split('\n').length - 1).toBe(4000);
    expect(l).not.toContain('[CHANGED]');
    expect(r.match(/\[CHANGED\]/g)?.length).toBe(3);
  });
});

describe('资产解码', () => {
  it('decodeAsset 还原 base64 为精确字节（PNG 签名）', () => {
    const bytes = decodeAsset('pngLogoL');
    expect(Array.from(bytes.slice(0, 8))).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    expect(bytes.length).toBeGreaterThan(0);
  });

  it('所有资产键均可解码且非空', () => {
    for (const key of Object.keys(DEMO_ASSETS) as (keyof typeof DEMO_ASSETS)[]) {
      expect(decodeAsset(key).length).toBeGreaterThan(0);
    }
  });
});

describe('演示目录路径与落盘清单', () => {
  it('两侧路径落在 <base>/demo 下且不产生重复斜杠', () => {
    const roots = demoRoots('/tmp/base');
    expect(roots.left).toBe('/tmp/base/demo/project-v1');
    expect(roots.right).toBe('/tmp/base/demo/project-v2');
    expect(demoRoots('/tmp/base/').left).toBe('/tmp/base/demo/project-v1');
  });

  it('落盘清单覆盖两侧全部文件，二进制项带 bytes、文本项带 contents', () => {
    const plan = demoFilePlan('/tmp/base');
    expect(plan.length).toBe(DEMO_LEFT_FILES.length + DEMO_RIGHT_FILES.length);
    expect(plan.every((f) => f.path.startsWith('/tmp/base/demo/'))).toBe(true);
    const bin = plan.find((f) => f.path.endsWith('images/logo.png'))!;
    expect(bin.bytes).toBeInstanceOf(Uint8Array);
    expect(bin.contents).toBeUndefined();
    const txt = plan.find((f) => f.path.endsWith('src/main.rs'))!;
    expect(typeof txt.contents).toBe('string');
    expect(txt.bytes).toBeUndefined();
  });
});

describe('seedDemoDirs', () => {
  it('写入文本与二进制文件并返回演示目录路径', async () => {
    const store = new MemoryFileStore();
    const roots = await seedDemoDirs('/tmp/base', store);
    expect(roots.left).toBe('/tmp/base/demo/project-v1');
    expect(await store.readFile('/tmp/base/demo/project-v1/src/main.rs')).toContain('hello, v1');
    const png = store.peekBinary('/tmp/base/demo/project-v1/images/logo.png');
    expect(png).toBeInstanceOf(Uint8Array);
    expect(Array.from(png!.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it('重复生成不残留 .bak（文本与二进制均写前先删）', async () => {
    const store = new MemoryFileStore();
    await seedDemoDirs('/tmp/base', store);
    await seedDemoDirs('/tmp/base', store);
    expect(await store.readFile('/tmp/base/demo/project-v1/src/main.rs.bak')).toBeNull();
    expect(store.peekBinary('/tmp/base/demo/project-v1/images/logo.png.bak')).toBeNull();
  });
});
