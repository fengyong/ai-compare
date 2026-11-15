#!/usr/bin/env node
// 生成全格式演示数据的二进制资产 → ui/src/lib/demoAssets.ts（base64 常量）。
//
// 为什么需要：演示落盘接口 settings_write 只接受 UTF-8 字符串，图片/ZIP/非 UTF-8
// 编码等资产无法用字符串表达。本脚本在「作者侧」一次性生成真实字节，前端 seedDemoDirs
// 经 settings_write_bytes 落盘。产物（demoAssets.ts）随仓库提交，故构建/运行无需复跑。
//
// 复跑：node scripts/gen-demo-assets.mjs
// 依赖：Node 内置 zlib（PNG/ZIP）；GB18030 经系统 python3（macOS/Linux 自带）。
//
// 注意：引擎 image crate 仅启用 png 特性（ai-core/Cargo.toml），故图片资产只用 PNG
// （BMP/JPG/GIF 会解码失败）。ZIP 用 store-only（method 0），标准格式，引擎 zip 可读。

import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import zlib from 'node:zlib';

// ---- CRC32（PNG/ZIP 共用）----
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

// ---- PNG（8-bit RGBA，filter 0，zlib deflate）----
function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const t = Buffer.from(type, 'ascii');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
}
function png(width, height, pixel) {
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  const raw = Buffer.alloc((width * 4 + 1) * height);
  let o = 0;
  for (let y = 0; y < height; y++) {
    raw[o++] = 0; // filter: none
    for (let x = 0; x < width; x++) {
      const [r, g, b, a] = pixel(x, y);
      raw[o++] = r;
      raw[o++] = g;
      raw[o++] = b;
      raw[o++] = a;
    }
  }
  const idat = zlib.deflateSync(raw, { level: 9 });
  return Buffer.concat([
    sig,
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', idat),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

// ---- ZIP（store-only，method 0）----
function zip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const e of entries) {
    const name = Buffer.from(e.name, 'utf8');
    const crc = crc32(e.data);
    const lh = Buffer.alloc(30);
    lh.writeUInt32LE(0x04034b50, 0);
    lh.writeUInt16LE(20, 4); // version needed
    lh.writeUInt16LE(0, 6); // flags
    lh.writeUInt16LE(0, 8); // method: store
    lh.writeUInt16LE(0, 10); // mod time
    lh.writeUInt16LE(0x21, 12); // mod date 1980-01-01
    lh.writeUInt32LE(crc, 14);
    lh.writeUInt32LE(e.data.length, 18);
    lh.writeUInt32LE(e.data.length, 22);
    lh.writeUInt16LE(name.length, 26);
    lh.writeUInt16LE(0, 28);
    const local = Buffer.concat([lh, name, e.data]);
    locals.push(local);
    const ch = Buffer.alloc(46);
    ch.writeUInt32LE(0x02014b50, 0);
    ch.writeUInt16LE(20, 4); // version made by
    ch.writeUInt16LE(20, 6); // version needed
    ch.writeUInt16LE(0, 8);
    ch.writeUInt16LE(0, 10);
    ch.writeUInt16LE(0, 12);
    ch.writeUInt16LE(0x21, 14);
    ch.writeUInt32LE(crc, 16);
    ch.writeUInt32LE(e.data.length, 20);
    ch.writeUInt32LE(e.data.length, 24);
    ch.writeUInt16LE(name.length, 28);
    ch.writeUInt16LE(0, 30);
    ch.writeUInt16LE(0, 32);
    ch.writeUInt16LE(0, 34);
    ch.writeUInt16LE(0, 36);
    ch.writeUInt32LE(0, 38);
    ch.writeUInt32LE(offset, 42);
    centrals.push(Buffer.concat([ch, name]));
    offset += local.length;
  }
  const cd = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(cd.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(0, 20);
  return Buffer.concat([...locals, cd, eocd]);
}
const ztext = (s) => Buffer.from(s, 'utf8');

// ---- 编码 ----
function gb18030(str) {
  return execFileSync(
    'python3',
    ['-c', 'import sys; sys.stdout.buffer.write(sys.stdin.buffer.read().decode("utf-8").encode("gb18030"))'],
    { input: Buffer.from(str, 'utf8') },
  );
}
function utf16le(str) {
  return Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(str, 'utf16le')]);
}

// ---- 像素图案 ----
const clamp = (v) => Math.max(0, Math.min(255, Math.round(v)));
function logo(bg) {
  return (x, y) => {
    const g = clamp(40 + (x / 64) * 120);
    let r = 20;
    let gg = g;
    let b = clamp(160 + (y / 64) * 80);
    const dx = x - 32;
    const dy = y - 32;
    if (dx * dx + dy * dy < 16 * 16) {
      r = 245;
      gg = 245;
      b = 245;
    }
    if (bg === 'right' && x > 44 && y > 44) {
      r = 220;
      gg = 60;
      b = 60;
    }
    return [r, gg, b, 255];
  };
}
const banner = (w) => (x, y) => [clamp((x / w) * 255), clamp(120 + (y / 40) * 100), clamp(255 - (x / w) * 200), 255];
const icon = (x, y) => {
  const border = x < 3 || y < 3 || x > 28 || y > 28;
  return border ? [60, 60, 70, 255] : [250, 200, 60, 255];
};
const onlyLeft = (x, y) => {
  const bar = Math.floor(x / 8);
  const h = [10, 24, 16, 32, 20, 40][bar % 6];
  return y > 48 - h ? [70, 130, 200, 255] : [235, 235, 240, 255];
};

// ---- 二进制 blob（256B，右侧 100..120 区段不同）----
function blob(side) {
  const b = Buffer.alloc(256);
  for (let i = 0; i < 256; i++) b[i] = (i * 7 + 3) & 0xff;
  if (side === 'right') for (let i = 100; i < 120; i++) b[i] = (i * 13 + 99) & 0xff;
  return b;
}

const assets = {
  // 图片（PNG）：logo 同尺寸内容差 / banner 不同尺寸 / icon 两侧相同 / onlyLeft 仅左
  pngLogoL: png(64, 64, logo('left')),
  pngLogoR: png(64, 64, logo('right')),
  pngBannerL: png(120, 40, banner(120)),
  pngBannerR: png(160, 40, banner(160)),
  pngIcon: png(32, 32, icon),
  pngOnlyLeft: png(48, 48, onlyLeft),
  // ZIP：左 readme/a/b/csv，右 readme(改)/a(同)/c(增,b删)/csv(改)
  zipDataL: zip([
    { name: 'readme.txt', data: ztext('archive v1\nline two\n') },
    { name: 'src/a.txt', data: ztext('alpha\n') },
    { name: 'src/b.txt', data: ztext('beta\n') },
    { name: 'data.csv', data: ztext('id,name\n1,alice\n2,bob\n') },
  ]),
  zipDataR: zip([
    { name: 'readme.txt', data: ztext('archive v2\nline two changed\nline three\n') },
    { name: 'src/a.txt', data: ztext('alpha\n') },
    { name: 'src/c.txt', data: ztext('gamma\n') },
    { name: 'data.csv', data: ztext('id,name\n1,alice\n2,bobby\n3,carol\n') },
  ]),
  // 编码：GB18030 与 UTF-16LE+BOM，两侧内容不同
  gbL: gb18030('演示项目说明\n版本：一\n作者：张三\n这是一个用于编码检测的中文文本文件。\n'),
  gbR: gb18030('演示项目说明\n版本：二\n作者：李四\n这是一个用于编码检测的中文文本文件，右侧有改动。\n新增一行。\n'),
  u16L: utf16le('Unicode demo\nversion: one\nline three\n'),
  u16R: utf16le('Unicode demo\nversion: two\nline three changed\nline four\n'),
  // 二进制（十六进制视图）
  blobL: blob('left'),
  blobR: blob('right'),
};

const out =
  '// 自动生成 —— 请勿手改。复跑：node scripts/gen-demo-assets.mjs\n' +
  '// 全格式演示数据的二进制资产（base64）。引擎 image crate 仅 png，故图片只用 PNG。\n' +
  '// 由 seedDemoDirs 经 settings_write_bytes 落盘（解码见 demoSeed.ts decodeAsset）。\n\n' +
  'export const DEMO_ASSETS = {\n' +
  Object.entries(assets)
    .map(([k, v]) => `  ${k}:\n    '${v.toString('base64')}',`)
    .join('\n') +
  '\n};\n\n' +
  'export type DemoAssetKey = keyof typeof DEMO_ASSETS;\n';

const dest = fileURLToPath(new URL('../ui/src/lib/demoAssets.ts', import.meta.url));
writeFileSync(dest, out);
const total = Object.values(assets).reduce((n, b) => n + b.length, 0);
console.log(`生成 ${Object.keys(assets).length} 个资产（共 ${total} 字节）→ ${dest}`);
for (const [k, v] of Object.entries(assets)) console.log(`  ${k}: ${v.length} B`);