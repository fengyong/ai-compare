#!/usr/bin/env node
/**
 * 演示资产真实引擎验收：把 demoAssets.ts 里**已提交的精确字节**落盘，逐视图驱动
 * ai-engine，证明全格式数据集真的能被引擎解码/比较（不只是单测里"写得出字节"）。
 *
 * 覆盖：picture(png 内容差异/尺寸差异/相同) · hex(blob 定位差异) · table(csv) ·
 * zip(归档路由 + 改名四态) · encoding(GB18030 / UTF-16LE+BOM 自动检测) ·
 * merge3(冲突) · text(CRLF/BOM/Unicode) · dir(混合格式四态)。
 *
 * 用法：node scripts/verify-demo-assets.mjs [引擎路径]
 * 退出码：0 通过；1 失败；2 引擎未构建。
 */
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const ENGINE = process.argv[2] ?? resolve(import.meta.dirname, '../../target/debug/ai-engine');
const ASSETS_TS = resolve(import.meta.dirname, '../src/lib/demoAssets.ts');
let failed = 0;
const ok = (cond, label, extra = '') => {
  console.log(`${cond ? '✓' : '✗'} ${label}${extra ? ` — ${extra}` : ''}`);
  if (!cond) failed++;
};

// ---------------- 从已提交的 demoAssets.ts 解析 base64（不重新生成，测的就是committed字节） ----------------
const src = readFileSync(ASSETS_TS, 'utf8');
const assets = {};
for (const m of src.matchAll(/(\w+):\s*'([A-Za-z0-9+/=]+)'/g)) {
  assets[m[1]] = Buffer.from(m[2], 'base64');
}
const need = [
  'pngLogoL', 'pngLogoR', 'pngBannerL', 'pngBannerR', 'pngIcon', 'pngOnlyLeft',
  'zipDataL', 'zipDataR', 'gbL', 'gbR', 'u16L', 'u16R', 'blobL', 'blobR',
];
for (const k of need) {
  if (!assets[k] || assets[k].length === 0) {
    console.error(`资产缺失或为空：${k}（先运行 node scripts/gen-demo-assets.mjs）`);
    process.exit(2);
  }
}
console.log(`已解析 ${Object.keys(assets).length} 个资产（来自 ${ASSETS_TS}）\n`);

// ---------------- 最小 NDJSON JSON-RPC 客户端 ----------------
class EngineClient {
  constructor() {
    if (!existsSync(ENGINE)) {
      console.error(`未找到引擎：${ENGINE}\n请先在仓库根执行：cargo build -p ai-engine`);
      process.exit(2);
    }
    this.child = spawn(ENGINE, ['--stdio'], { stdio: ['pipe', 'pipe', 'inherit'] });
    this.buf = '';
    this.nextId = 1;
    this.pending = new Map();
    this.handlers = new Map();
    this.exit = new Promise((res) => this.child.on('exit', (code) => res(code)));
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', (c) => this.#onData(c));
    this.ready = new Promise((res, rej) => {
      this.child.on('spawn', () => res());
      this.child.on('error', rej);
    });
  }
  #onData(chunk) {
    this.buf += chunk;
    let idx;
    while ((idx = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, idx).trim();
      this.buf = this.buf.slice(idx + 1);
      if (!line) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
        const p = this.pending.get(msg.id);
        if (p) {
          this.pending.delete(msg.id);
          msg.error ? p.reject(new Error(`RPC ${msg.error.code}: ${msg.error.message}`)) : p.resolve(msg.result);
        }
      } else if (msg.method) {
        for (const fn of this.handlers.get(msg.method) ?? []) fn(msg.params);
      }
    }
  }
  request(method, params, timeoutMs = 15000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`超时：${method}`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (v) => (clearTimeout(t), resolve(v)),
        reject: (e) => (clearTimeout(t), reject(e)),
      });
      this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    });
  }
  on(method, fn) {
    if (!this.handlers.has(method)) this.handlers.set(method, []);
    this.handlers.get(method).push(fn);
  }
  async close() {
    this.child.stdin.end();
    const timer = new Promise((res) => setTimeout(() => res('timeout'), 5000));
    const code = await Promise.race([this.exit, timer]);
    if (code === 'timeout') this.child.kill('SIGKILL');
    return code;
  }
}

// ---------------- 落盘夹具 ----------------
const dir = mkdtempSync(join(tmpdir(), 'aic-demo-verify-'));
const L = join(dir, 'L');
const R = join(dir, 'R');
mkdirSync(L, { recursive: true });
mkdirSync(R, { recursive: true });
const put = (root, rel, data) => {
  const p = join(root, rel);
  mkdirSync(join(p, '..'), { recursive: true });
  writeFileSync(p, data);
  return p;
};
// 图片
put(L, 'logo.png', assets.pngLogoL); put(R, 'logo.png', assets.pngLogoR);
put(L, 'banner.png', assets.pngBannerL); put(R, 'banner.png', assets.pngBannerR);
put(L, 'icon.png', assets.pngIcon); put(R, 'icon.png', assets.pngIcon);
put(L, 'only-left.png', assets.pngOnlyLeft);
// hex / 二进制
put(L, 'blob.dat', assets.blobL); put(R, 'blob.dat', assets.blobR);
put(R, 'right-only.dat', assets.blobR);
// zip
put(L, 'data.zip', assets.zipDataL); put(R, 'data.zip', assets.zipDataR);
// 编码
put(L, 'gb.txt', assets.gbL); put(R, 'gb.txt', assets.gbR);
put(L, 'u16.txt', assets.u16L); put(R, 'u16.txt', assets.u16R);
// 表格
put(L, 'users.csv', 'id,name,role\n1,alice,admin\n2,bob,user\n');
put(R, 'users.csv', 'id,name,role\n1,alice,admin\n2,bob,editor\n3,carol,user\n');
// 文本四态 + 变体
put(L, 'same.txt', 'alpha\nbeta\n'); put(R, 'same.txt', 'alpha\nbeta\n');
put(L, 'diff.txt', 'line1\nOLD\n'); put(R, 'diff.txt', 'line1\nNEW\nextra\n');
put(L, 'crlf.txt', 'a\r\nb\r\n'); put(R, 'crlf.txt', 'a\nb\n');
put(L, 'bom.txt', '﻿one\ntwo\n'); put(R, 'bom.txt', 'one\ntwo\n');
put(L, 'unicode.txt', '中文 🎉\n'); put(R, 'unicode.txt', '中文 🎉\n');
// merge3
const m3base = put(L, 'merge3/base.md', '# 配置\n\nname = demo\nmode = fast\nretry = 3\n');
const m3left = put(L, 'merge3/left.md', '# 配置\n\nname = demo-left\nmode = fast\nretry = 5\n');
const m3right = put(L, 'merge3/right.md', '# 配置\n\nname = demo-right\nmode = safe\nretry = 3\n');

const client = new EngineClient();
await client.ready;
console.log(`引擎已启动: ${ENGINE}\n`);

const deltas = [];
client.on('dir/delta', (p) => deltas.push(p));
const rowsOf = (sid) => {
  const m = new Map();
  for (const d of deltas) if (d.sessionId === sid) for (const u of d.upserts) m.set(u.id, u);
  return m;
};
const waitRows = async (sid, n) => {
  const until = Date.now() + 5000;
  while (rowsOf(sid).size < n && Date.now() < until) await new Promise((r) => setTimeout(r, 50));
  return rowsOf(sid);
};
const CJK = /[一-鿿]/;

try {
  const init = await client.request('initialize', {
    clientInfo: { name: 'demo-verify', version: '0.1.0' },
    protocolVersion: '1.0',
  });
  ok(!!init?.capabilities?.dirCompare, 'initialize 能力位', `engine=${init.engine?.name} ${init.engine?.version}`);

  // ---- picture ----
  const logo = await client.request('picture/open', { sessionId: 'v-logo', left: join(L, 'logo.png'), right: join(R, 'logo.png') });
  ok(logo.leftWidth === 64 && logo.leftHeight === 64 && logo.rightWidth === 64 && logo.rightHeight === 64, 'picture logo 尺寸 64x64', `${logo.leftWidth}x${logo.leftHeight}`);
  ok(Number(logo.diffPixels) > 0 && logo.similarityPct < 100, 'picture logo 内容差异', `diffPixels=${logo.diffPixels} sim=${logo.similarityPct}`);
  const banner = await client.request('picture/open', { sessionId: 'v-banner', left: join(L, 'banner.png'), right: join(R, 'banner.png') });
  ok(banner.leftWidth === 120 && banner.rightWidth === 160 && banner.leftHeight === 40 && banner.rightHeight === 40, 'picture banner 尺寸差异 120x40 vs 160x40', `L=${banner.leftWidth}x${banner.leftHeight} R=${banner.rightWidth}x${banner.rightHeight}`);
  ok(banner.width === 160 && Number(banner.diffPixels) > 0, 'picture banner 画布取最大宽且判异', `canvas=${banner.width} diff=${banner.diffPixels}`);
  const icon = await client.request('picture/open', { sessionId: 'v-icon', left: join(L, 'icon.png'), right: join(R, 'icon.png') });
  ok(Number(icon.diffPixels) === 0 && icon.similarityPct === 100, 'picture icon 全同', `sim=${icon.similarityPct}`);
  for (const s of ['v-logo', 'v-banner', 'v-icon']) await client.request('session/close', { sessionId: s });

  // ---- hex ----
  const hex = await client.request('hex/open', { sessionId: 'v-hex', left: join(L, 'blob.dat'), right: join(R, 'blob.dat') });
  ok(Number(hex.leftSize) === 256 && Number(hex.rightSize) === 256, 'hex blob 两侧 256B', `${hex.leftSize}/${hex.rightSize}`);
  ok(hex.blockCount > 1, 'hex blob 存在差异块', `blockCount=${hex.blockCount}`);
  const hexDiff = await client.request('hex/bytes', { sessionId: 'v-hex', side: 'left', offset: 96, length: 32 });
  const diffStatus = hexDiff.lines.map((l) => l.status).join('');
  ok(diffStatus.includes('x'), 'hex 差异定位在 offset 100..120 区段', diffStatus);
  const hexSame = await client.request('hex/bytes', { sessionId: 'v-hex', side: 'left', offset: 0, length: 16 });
  ok(hexSame.lines.every((l) => /^=+$/.test(l.status)), 'hex 头部 16B 全同', hexSame.lines.map((l) => l.status).join(''));
  await client.request('session/close', { sessionId: 'v-hex' });

  // ---- table ----
  const tbl = await client.request('table/open', { sessionId: 'v-table', left: join(L, 'users.csv'), right: join(R, 'users.csv'), options: null });
  ok(tbl.leftRowCount === 2 && tbl.rightRowCount === 3, 'table csv 数据行数 左2/右3(不含表头)', `${tbl.leftRowCount}/${tbl.rightRowCount}`);
  ok(tbl.columns.length === 3, 'table csv 三列', `cols=${tbl.columns.length}`);
  const tblSecs = await client.request('table/sections', { sessionId: 'v-table', offset: 0, count: 10 });
  ok(tblSecs.rows.length > 0, 'table/sections 可取数据', `rows=${tblSecs.rows.length}`);
  await client.request('session/close', { sessionId: 'v-table' });

  // ---- zip 路由 + 改名四态 ----
  const zipTextL = `zip://${join(L, 'data.zip')}!readme.txt`;
  const zipTextR = `zip://${join(R, 'data.zip')}!readme.txt`;
  await client.request('text/open', { sessionId: 'v-zip-text', left: zipTextL, right: zipTextR });
  const zipLines = await client.request('text/lines', { sessionId: 'v-zip-text', side: 'left', start: 0, count: 10 });
  ok(Array.isArray(zipLines.lines) && zipLines.lines.length > 0, 'zip!readme.txt 文本路由读到条目', JSON.stringify(zipLines.lines).slice(0, 80));
  await client.request('session/close', { sessionId: 'v-zip-text' });

  const zipDirOpen = await client.request('dir/open', {
    sessionId: 'v-zip-dir', left: `zip://${join(L, 'data.zip')}!/`, right: `zip://${join(R, 'data.zip')}!/`,
    depth: null, criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
  });
  const zipRows = await waitRows('v-zip-dir', zipDirOpen.rowCount);
  const zipIds = [...zipRows.keys()];
  const bRow = zipIds.find((id) => id.endsWith('b.txt'));
  const cRow = zipIds.find((id) => id.endsWith('c.txt'));
  ok(zipDirOpen.rowCount >= 4, 'zip 归档目录比较行数 >=4', `rowCount=${zipDirOpen.rowCount}`);
  ok(bRow && zipRows.get(bRow).status === 'leftOnly', 'zip 内 src/b.txt 仅左（改名源）', `${bRow}=${zipRows.get(bRow)?.status}`);
  ok(cRow && zipRows.get(cRow).status === 'rightOnly', 'zip 内 src/c.txt 仅右（改名目标）', `${cRow}=${zipRows.get(cRow)?.status}`);
  await client.request('session/close', { sessionId: 'v-zip-dir' });

  // ---- encoding ----
  const gb = await client.request('text/open', { sessionId: 'v-gb', left: join(L, 'gb.txt'), right: join(R, 'gb.txt') });
  ok(gb.leftEncoding !== 'UTF-8', 'GB18030 自动检测出非 UTF-8 编码', gb.leftEncoding);
  const gbLines = await client.request('text/lines', { sessionId: 'v-gb', side: 'left', start: 0, count: 5 });
  ok(CJK.test(gbLines.lines.join('')), 'GB18030 内容正确解码出中文', JSON.stringify(gbLines.lines).slice(0, 60));
  await client.request('session/close', { sessionId: 'v-gb' });

  const u16 = await client.request('text/open', { sessionId: 'v-u16', left: join(L, 'u16.txt'), right: join(R, 'u16.txt') });
  ok(u16.leftEncoding === 'UTF-16LE', 'UTF-16LE+BOM 自动检测', u16.leftEncoding);
  const u16Lines = await client.request('text/lines', { sessionId: 'v-u16', side: 'left', start: 0, count: 5 });
  ok(CJK.test(u16Lines.lines.join('')) && !u16Lines.lines[0].startsWith('﻿'), 'UTF-16 解码出中文且 BOM 不进首行', JSON.stringify(u16Lines.lines).slice(0, 60));
  await client.request('session/close', { sessionId: 'v-u16' });

  // ---- merge3 ----
  const m3 = await client.request('text/merge3/open', { sessionId: 'v-m3', base: m3base, left: m3left, right: m3right, options: null });
  ok(m3.conflictCount >= 1, 'merge3 name 两侧改 → 至少 1 冲突', `conflicts=${m3.conflictCount}`);
  const m3secs = await client.request('text/merge3/sections', { sessionId: 'v-m3', offset: 0, count: 100 });
  ok(m3secs.chunks.some((c) => c.origin === 'conflict'), 'merge3 存在冲突块');
  ok(m3secs.chunks.some((c) => c.origin !== 'conflict' && c.unresolved === false), 'merge3 存在可自动合并块（mode/retry 单侧改）');
  await client.request('session/close', { sessionId: 'v-m3' });

  // ---- text 变体 ----
  const crlf = await client.request('text/open', { sessionId: 'v-crlf', left: join(L, 'crlf.txt'), right: join(R, 'crlf.txt') });
  ok(crlf.leftLineCount === 2 && crlf.rightLineCount === 2, 'CRLF vs LF 行数一致', `${crlf.leftLineCount}/${crlf.rightLineCount}`);
  await client.request('session/close', { sessionId: 'v-crlf' });
  const bom = await client.request('text/open', { sessionId: 'v-bom', left: join(L, 'bom.txt'), right: join(R, 'bom.txt') });
  ok(bom.leftLineCount === 2 && bom.rightLineCount === 2, 'BOM vs 无 BOM 可比较', `${bom.leftLineCount}/${bom.rightLineCount}`);
  await client.request('session/close', { sessionId: 'v-bom' });
  const uni = await client.request('text/open', { sessionId: 'v-uni', left: join(L, 'unicode.txt'), right: join(R, 'unicode.txt') });
  const uniSecs = await client.request('text/sections', { sessionId: 'v-uni', offset: 0, count: 10 });
  ok(uniSecs.sections.every((s) => s.kind === 'equal'), 'Unicode 相同文件全 equal', `leftEncoding=${uni.leftEncoding}`);
  await client.request('session/close', { sessionId: 'v-uni' });

  // ---- dir 混合格式四态 ----
  const dirOpen = await client.request('dir/open', {
    sessionId: 'v-dir', left: L, right: R, depth: null, criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
  });
  const drows = await waitRows('v-dir', dirOpen.rowCount);
  const st = (id) => drows.get(id)?.status;
  ok(st('same.txt') === 'same', 'dir same.txt → same', st('same.txt'));
  ok(st('diff.txt') === 'contentDiff', 'dir diff.txt → contentDiff', st('diff.txt'));
  ok(st('only-left.png') === 'leftOnly', 'dir only-left.png → leftOnly', st('only-left.png'));
  ok(st('right-only.dat') === 'rightOnly', 'dir right-only.dat → rightOnly', st('right-only.dat'));
  ok(st('logo.png') === 'contentDiff', 'dir logo.png(二进制差异) → contentDiff', st('logo.png'));
  ok(st('icon.png') === 'same', 'dir icon.png(二进制相同) → same', st('icon.png'));
  ok(st('users.csv') === 'contentDiff', 'dir users.csv → contentDiff', st('users.csv'));
  await client.request('session/close', { sessionId: 'v-dir' });

  ok((await client.request('shutdown', null)) === null, 'shutdown 应答 null');
  const exitCode = await client.close();
  ok(exitCode === 0, '引擎进程随 shutdown 退出', `exit=${exitCode}`);
} catch (e) {
  ok(false, `异常中断: ${e.message}`);
  await client.close();
} finally {
  rmSync(dir, { recursive: true, force: true });
}

console.log(failed === 0 ? '\n✅ 演示资产真实引擎验收：全部通过' : `\n❌ 演示资产验收：${failed} 项失败`);
process.exit(failed === 0 ? 0 : 1);