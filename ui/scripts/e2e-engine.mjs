#!/usr/bin/env node
/**
 * 真实引擎端到端验收：ui ⇄ ai-engine 的 M1 walking-skeleton 闭环。
 *
 * 直接 spawn `../target/debug/ai-engine --stdio`（workspace 根 target），
 * 走完整协议链路（docs/03 §2.3）：
 *   initialize → dir/open → dir/delta 收批 → 状态断言
 *   → text/open → text/sections → text/merge3/* → dir/mergeScan|mergePlan|mergeExecute
 *   → report/generate → session/close → shutdown → 进程退出
 *
 * 用法：pnpm e2e:engine（或 node scripts/e2e-engine.mjs [引擎路径]）
 * 退出码：0 = 通过；1 = 失败。引擎未构建时给出构建指引并退出 2。
 */
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const ENGINE = process.argv[2] ?? resolve(import.meta.dirname, '../../target/debug/ai-engine');
let failed = 0;

const ok = (cond, label, extra = '') => {
  console.log(`${cond ? '✓' : '✗'} ${label}${extra ? ` — ${extra}` : ''}`);
  if (!cond) failed++;
};

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
    this.pending = new Map(); // id -> {resolve, reject}
    this.handlers = new Map(); // method -> [fn]
    this.exit = new Promise((res) => this.child.on('exit', (code) => res(code)));
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', (chunk) => this.#onData(chunk));
    this.child.on('spawn', () => this.readyResolve());
    this.ready = new Promise((res, rej) => {
      this.readyResolve = res;
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
      } catch (e) {
        console.error('非 NDJSON 行：', line.slice(0, 200));
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
    const line = JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n';
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`超时：${method}`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (v) => (clearTimeout(t), resolve(v)),
        reject: (e) => (clearTimeout(t), reject(e)),
      });
      this.child.stdin.write(line);
    });
  }

  notify(method, params) {
    this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method, params }) + '\n');
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

// ---------------- 最小 zip 夹具（stored，无压缩；避免依赖外部 zip 命令） ----------------
const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[i] = c;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = -1;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};

function buildStoredZip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, data] of entries) {
    const nb = Buffer.from(name, 'utf8');
    const crc = crc32(data);
    const lh = Buffer.alloc(30);
    lh.writeUInt32LE(0x04034b50, 0);
    lh.writeUInt16LE(20, 4);
    lh.writeUInt16LE(0, 6);
    lh.writeUInt16LE(0, 8); // stored
    lh.writeUInt32LE(crc, 14);
    lh.writeUInt32LE(data.length, 18);
    lh.writeUInt32LE(data.length, 22);
    lh.writeUInt16LE(nb.length, 26);
    locals.push(lh, nb, data);
    const ch = Buffer.alloc(46);
    ch.writeUInt32LE(0x02014b50, 0);
    ch.writeUInt16LE(20, 4);
    ch.writeUInt16LE(20, 6);
    ch.writeUInt32LE(crc, 16);
    ch.writeUInt32LE(data.length, 20);
    ch.writeUInt32LE(data.length, 24);
    ch.writeUInt16LE(nb.length, 28);
    ch.writeUInt32LE(offset, 42);
    centrals.push(ch, nb);
    offset += lh.length + nb.length + data.length;
  }
  const cd = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(cd.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, eocd]);
}

/** 只读中心目录取条目名（够用来断言"归档里多了哪个文件"） */
function readStoredZipNames(buf) {
  const eocd = buf.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const names = [];
  for (let i = 0; i < count; i++) {
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    names.push(buf.subarray(p + 46, p + 46 + nameLen).toString('utf8'));
    p += 46 + nameLen + extraLen + commentLen;
  }
  return names;
}

// ---------------- 场景语料 ----------------

const dir = mkdtempSync(join(tmpdir(), 'aic-e2e-'));
const L = join(dir, 'L');
const R = join(dir, 'R');
for (const d of [L, R]) {
  (await import('node:fs')).mkdirSync(d);
}
writeFileSync(join(L, 'same.txt'), 'alpha\nbeta\ngamma\n');
writeFileSync(join(R, 'same.txt'), 'alpha\nbeta\ngamma\n');
writeFileSync(join(L, 'diff.txt'), 'line1\nline2 OLD\nline3\nline4\n');
writeFileSync(join(R, 'diff.txt'), 'line1\nline2 NEW\nline3\nline4\nline5 extra\n');
writeFileSync(join(L, 'leftOnly.txt'), 'only on left\n');
writeFileSync(join(R, 'rightOnly.txt'), 'only on right\n');
// P7 三向合并语料：base 5 行；左改两处；右改两处 + 尾部插入 1 行
writeFileSync(join(dir, 'base.txt'), 'h\na\nb\nc\nt\n');
writeFileSync(join(dir, 'left.txt'), 'h\nL1\nb\nL2\nt\n');
writeFileSync(join(dir, 'right.txt'), 'h\nR1\nb\nR2\nt\nR3\n');

// R3-07 文件夹三向合并语料：三方目录（一致/单侧改/冲突/孤立/删除跟随）
const MB = join(dir, 'MB');
const ML = join(dir, 'ML');
const MR = join(dir, 'MR');
for (const d of [MB, ML, MR]) mkdirSync(d);
const put = (root, name, text) => writeFileSync(join(root, name), text);
for (const root of [MB, ML, MR]) put(root, 'same.txt', 's\n');
put(MB, 'lchg.txt', 'old\n'); put(ML, 'lchg.txt', 'new\n'); put(MR, 'lchg.txt', 'old\n');
put(MB, 'rchg.txt', 'old\n'); put(ML, 'rchg.txt', 'old\n'); put(MR, 'rchg.txt', 'new\n');
put(MB, 'conf.txt', 'b\n'); put(ML, 'conf.txt', 'l\n'); put(MR, 'conf.txt', 'r\n');
put(ML, 'lonly.txt', 'L\n');
put(MB, 'del.txt', 'x\n'); put(ML, 'del.txt', 'x\n');

const client = new EngineClient();
await client.ready;
console.log(`引擎已启动: ${ENGINE}\n`);

// dir/delta 通知收集（引擎先答 dir/open，再异步分批推送）
const deltas = [];
client.on('dir/delta', (p) => deltas.push(p));
const rowsCount = () => deltas.reduce((n, d) => n + d.upserts.length, 0);

try {
  // 1. initialize
  const init = await client.request('initialize', {
    clientInfo: { name: 'e2e-engine', version: '0.1.0' },
    protocolVersion: '1.0',
  });
  ok(!!init?.capabilities?.dirCompare && !!init?.capabilities?.textCompare, 'initialize 返回能力位', `engine=${init.engine?.name} ${init.engine?.version}`);
  ok(init?.capabilities?.textEdit === true, 'initialize 宣告 textEdit 能力位（R3-18）', `textEdit=${init?.capabilities?.textEdit}`);

  // 2. dir/open + 收 delta 批次（收集器已在上方注册）
  const dirOpen = await client.request('dir/open', {
    sessionId: 'e2e-dir-1',
    left: L,
    right: R,
    depth: null,
    criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
  });
  ok(dirOpen?.rowCount > 0 && dirOpen?.scanState === 'completed', 'dir/open 应答', `rowCount=${dirOpen?.rowCount} scanState=${dirOpen?.scanState}`);

  // 引擎先答 dir/open，再分批推 dir/delta：等行数收齐（超时 5s）
  const deadline = Date.now() + 5000;
  while (rowsCount() < dirOpen.rowCount && Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 50));
  }
  ok(deltas.length > 0 && deltas[deltas.length - 1].batch.hasMore === false, 'dir/delta 批次收齐（末批 hasMore=false）', `批次=${deltas.length}`);

  // 合并行并断言状态判定
  const rows = new Map();
  for (const d of deltas) for (const u of d.upserts) rows.set(u.id, u);
  const stat = (id) => rows.get(id)?.status;
  ok(stat('same.txt') === 'same', 'same.txt 判定为 same', stat('same.txt'));
  ok(stat('diff.txt') === 'contentDiff', 'diff.txt 判定为 contentDiff', stat('diff.txt'));
  ok(stat('leftOnly.txt') === 'leftOnly', 'leftOnly.txt 判定为 leftOnly', stat('leftOnly.txt'));
  ok(stat('rightOnly.txt') === 'rightOnly', 'rightOnly.txt 判定为 rightOnly', stat('rightOnly.txt'));
  const revs = deltas.map((d) => Number(d.rev));
  ok([...revs].sort((a, b) => a - b).every((v, i) => v === revs[i]), 'rev 单调递增');

  // 2b. R3-16：dir/compareContent 三方式 + 五态结果（FR-DIR-011/012）
  ok(
    init?.capabilities?.dirCompareContent === true,
    'initialize 能力位 dirCompareContent=true（R3-16）',
    String(init?.capabilities?.dirCompareContent)
  );
  const CCL = join(dir, 'CCL');
  const CCR = join(dir, 'CCR');
  mkdirSync(CCL);
  mkdirSync(CCR);
  const ccPut = (root, name, text) => writeFileSync(join(root, name), text);
  ccPut(CCL, 'identical.txt', 'x\n');
  ccPut(CCR, 'identical.txt', 'x\n');
  ccPut(CCL, 'eol.txt', 'a\r\nb\r\n');
  ccPut(CCR, 'eol.txt', 'a\nb\n');
  ccPut(CCL, 'case.txt', 'Hello\n');
  ccPut(CCR, 'case.txt', 'hello\n');
  ccPut(CCL, 'ws.txt', 'a \n');
  ccPut(CCR, 'ws.txt', 'a\n');
  ccPut(CCL, 'real.txt', 'a\n');
  ccPut(CCR, 'real.txt', 'b\n');
  ccPut(CCL, 'orphan.txt', 'only left\n');
  const ccDeltas = [];
  client.on('dir/delta', (p) => {
    if (p.sessionId === 'e2e-dir-cc') ccDeltas.push(p);
  });
  const ccOpen = await client.request('dir/open', {
    sessionId: 'e2e-dir-cc',
    left: CCL,
    right: CCR,
    depth: null,
    // 与真实 UI 同口径：lazyScan 启用 P2 会话模型（dir/compareContent 挂在该模型上）
    lazyScan: true,
    // never：扫描只做快速判定，内容结论完全由手动比较产生（FR-DIR-011）
    criteria: { mtimeToleranceSec: 2, contentCompare: 'never', contentMethod: 'rulesBased' },
    rules: { eolStyleUnimportant: true, caseUnimportant: true, trailingWsUnimportant: true },
  });
  ok(ccOpen?.rowCount === 6, 'dir/open（内容比较语料）行数', `rowCount=${ccOpen?.rowCount}`);
  const ccRows = () => {
    const m = new Map();
    for (const d of ccDeltas) for (const u of d.upserts) m.set(u.id, u);
    return m;
  };
  const ccDeadline = Date.now() + 5000;
  while (ccRows().size < ccOpen.rowCount && Date.now() < ccDeadline) {
    await new Promise((r) => setTimeout(r, 50));
  }
  ok(ccRows().size === 6, 'dir/delta（内容比较语料）收齐', `rows=${ccRows().size}`);
  ok(ccRows().get('orphan.txt')?.status === 'leftOnly', '孤立项为 leftOnly（不参与内容比较）');

  const ccRules = await client.request('dir/compareContent', {
    sessionId: 'e2e-dir-cc',
    ids: [],
    method: 'rulesBased',
  });
  const ccBy = Object.fromEntries((ccRules?.results ?? []).map((r) => [r.id, r]));
  ok(Object.keys(ccBy).length === 5, 'ids 空 = 比较全部已配对文件（孤立项不参与）', `n=${Object.keys(ccBy).length}`);
  ok(ccBy['identical.txt']?.result === 'binarySame', '字节全等 → binarySame', ccBy['identical.txt']?.result);
  ok(ccBy['eol.txt']?.result === 'rulesSame', '仅行尾风格差异 → rulesSame（规则比较不按 size 短路）', ccBy['eol.txt']?.result);
  ok(ccBy['real.txt']?.result === 'importantDiff', '真实内容差异 → importantDiff', ccBy['real.txt']?.result);
  ok(
    ccBy['case.txt']?.result === 'unimportantDiff' && ccBy['case.txt']?.status === 'similar',
    '仅大小写差异 → unimportantDiff + 行状态 similar',
    `${ccBy['case.txt']?.result}/${ccBy['case.txt']?.status}`
  );
  ok(ccBy['ws.txt']?.result === 'unimportantDiff', '仅尾随空白差异 → unimportantDiff', ccBy['ws.txt']?.result);
  ok(ccRules?.stats?.similar === 2, 'DirStats.similar 新计数器随比较结果更新', String(ccRules?.stats?.similar));
  ok(ccBy['real.txt']?.status === 'contentDiff', 'importantDiff → 行状态 contentDiff', ccBy['real.txt']?.status);
  ok(ccBy['eol.txt']?.status === 'same', 'rulesSame → 行状态 same', ccBy['eol.txt']?.status);
  // 比较结论必须经 dir/delta 回推（UI 只认 delta，不认应答体）
  const ccAfter = Date.now() + 5000;
  while (ccRows().get('real.txt')?.status !== 'contentDiff' && Date.now() < ccAfter) {
    await new Promise((r) => setTimeout(r, 50));
  }
  ok(ccRows().get('real.txt')?.status === 'contentDiff', '比较后行状态经 dir/delta 回推', ccRows().get('real.txt')?.status);
  ok(ccRows().get('eol.txt')?.status === 'same', 'rulesSame 行回推为 same', ccRows().get('eol.txt')?.status);

  const ccBin = await client.request('dir/compareContent', {
    sessionId: 'e2e-dir-cc',
    ids: ['eol.txt', 'identical.txt'],
    method: 'binary',
  });
  const binBy = Object.fromEntries((ccBin?.results ?? []).map((r) => [r.id, r]));
  ok(
    binBy['eol.txt']?.result === 'binaryDiff' && binBy['identical.txt']?.result === 'binarySame',
    '二进制方式：行尾差异判异、字节全等判同',
    JSON.stringify(ccBin?.results)
  );
  ok(binBy['eol.txt']?.result !== ccBy['eol.txt']?.result, '三方式不可互换（FR-DIR-011）');
  const ccCrc = await client.request('dir/compareContent', {
    sessionId: 'e2e-dir-cc',
    ids: ['eol.txt'],
    method: 'crc',
  });
  ok(ccCrc?.results?.[0]?.result === 'binaryDiff', 'CRC 方式同样判异', ccCrc?.results?.[0]?.result);
  const ccDefault = await client.request('dir/compareContent', { sessionId: 'e2e-dir-cc', ids: ['eol.txt'] });
  ok(
    ccDefault?.results?.[0]?.result === ccBy['eol.txt']?.result,
    '省略 method 取会话判据的方法（设置 → dir/open → 比较）',
    ccDefault?.results?.[0]?.result
  );
  let ccNotFound = '';
  try {
    await client.request('dir/compareContent', { sessionId: 'no-such-session', ids: [], method: null });
  } catch (e) {
    ccNotFound = String(e.message);
  }
  ok(ccNotFound.includes('410'), '未知会话 → 410', ccNotFound);
  await client.request('session/close', { sessionId: 'e2e-dir-cc' });

  // 2c. R3-17：子会话保存 → 父目录行回传（FR-SES-004）
  // 回传走父会话既有重扫入口 dir/filter rescan:true（与右键「刷新」同一条路径，
  // UI 侧由 parentRescanParams 构造）；重扫后引擎推 dir/delta，父会话缓存是
  // 会话级订阅，父标签不在前台也追平。
  // 父会话必须带 lazyScan（真实 UI 恒传）：dir/filter 只对 P2 会话模型开放。
  writeFileSync(join(L, 'wb.txt'), 'old\n');
  writeFileSync(join(R, 'wb.txt'), 'new content\n');
  const WB_PARENT = 'e2e-dir-wb';
  const wbFilter = {
    sessionId: WB_PARENT,
    nameFilter: null,
    otherFilter: null,
    displayFilter: null,
    suppress: null,
    flat: null,
    rescan: true,
  };
  const wbSince = (from) => {
    const m = new Map();
    for (const d of deltas.slice(from)) {
      if (d.sessionId !== WB_PARENT) continue;
      for (const u of d.upserts) m.set(u.id, u);
    }
    return m;
  };
  const wbWait = async (from, pred) => {
    const until = Date.now() + 5000;
    while (Date.now() < until) {
      if (pred(wbSince(from))) return wbSince(from);
      await new Promise((r) => setTimeout(r, 50));
    }
    return wbSince(from);
  };
  const wbMark0 = deltas.length;
  await client.request('dir/open', {
    sessionId: WB_PARENT,
    left: L,
    right: R,
    depth: null,
    lazyScan: true,
    criteria: { mtimeToleranceSec: 2, contentCompare: 'auto' },
  });
  const wbOpen = await wbWait(wbMark0, (m) => m.get('wb.txt')?.status === 'contentDiff');
  ok(
    wbOpen.get('wb.txt')?.status === 'contentDiff',
    'R3-17 父会话初扫判定 wb.txt 为 contentDiff',
    wbOpen.get('wb.txt')?.status,
  );
  await client.request('text/open', {
    sessionId: 'e2e-wb-child',
    left: join(L, 'wb.txt'),
    right: join(R, 'wb.txt'),
    options: null,
  });
  await client.request('text/edit', {
    sessionId: 'e2e-wb-child',
    edits: [{ side: 'left', start: 0, end: 1, text: 'new content' }],
  });
  const wbSave = await client.request('text/save', { sessionId: 'e2e-wb-child' });
  ok(
    wbSave?.saved?.length === 1 && readFileSync(join(L, 'wb.txt'), 'utf8') === 'new content\n',
    'R3-17 子会话保存把左侧改成与右侧一致',
    JSON.stringify(wbSave?.saved?.map((e) => e.path)),
  );
  // 回传前父行仍是 contentDiff（证明追平不是"顺便"发生的，必须显式重扫）
  ok(
    wbSince(wbMark0).get('wb.txt')?.status === 'contentDiff',
    'R3-17 未回传前父行保持旧判定',
    wbSince(wbMark0).get('wb.txt')?.status,
  );
  const wbMark1 = deltas.length;
  await client.request('dir/filter', wbFilter);
  const wbBack = await wbWait(wbMark1, (m) => m.get('wb.txt')?.status === 'same');
  ok(
    wbBack.get('wb.txt')?.status === 'same',
    'R3-17 回传后父目录行追平为 same',
    wbBack.get('wb.txt')?.status,
  );
  ok(
    Number(wbBack.get('wb.txt')?.left?.size) === 12 &&
      Number(wbOpen.get('wb.txt')?.left?.size) === 4,
    'R3-17 回传同时刷新元数据（左侧 size 4 → 12）',
    `${wbOpen.get('wb.txt')?.left?.size} → ${wbBack.get('wb.txt')?.left?.size}`,
  );
  await client.request('session/close', { sessionId: 'e2e-wb-child' });
  await client.request('session/close', { sessionId: WB_PARENT });

  // 3. text/open + text/sections
  const text = await client.request('text/open', {
    sessionId: 'e2e-text-1',
    left: join(L, 'diff.txt'),
    right: join(R, 'diff.txt'),
    options: null,
  });
  ok(text?.leftLineCount === 4 && text?.rightLineCount === 5, 'text/open 行数', `left=${text?.leftLineCount} right=${text?.rightLineCount}`);

  const secs = await client.request('text/sections', { sessionId: 'e2e-text-1', offset: 0, count: 100 });
  const kinds = (secs?.sections ?? []).map((s) => s.kind);
  ok(
    secs?.total > 0 && secs?.hasMore === false && kinds.includes('equal') && (kinds.includes('change') || kinds.includes('delete') || kinds.includes('insert')),
    'text/sections 差异结构合理',
    `total=${secs?.total} kinds=${kinds.join(',')}`
  );
  const hasFull = (secs?.sections ?? []).some((s) => s.inline === 'full' && s.left && s.right);
  if (init?.capabilities?.inlineDiff === false) {
    // 引擎 P0 能力位 inlineDiff=false：行内区间 P1 开放，此断言降级为提示
    console.log(`ℹ 行内差异区间：引擎 P0 未开放（inlineDiff=false），已收 inline 状态=[${[...new Set((secs?.sections ?? []).map((s) => s.inline))].join(',')}]，P1 补验`);
  } else {
    ok(hasFull, '替换段带行内差异区间（inline=full）');
  }

  // 3b. R3-18：text/edit 编辑增量重 diff —— 重要性必须由引擎规则判定，
  //     不得像客户端全量重算那样把编辑后的所有差异一律置为 important。
  writeFileSync(join(L, 'ws.txt'), 'alpha\nbeta one  \ngamma\n');
  writeFileSync(join(R, 'ws.txt'), 'alpha\nbeta one\ngamma\n');
  const wsOpen = await client.request('text/open', {
    sessionId: 'e2e-text-edit-1',
    left: join(L, 'ws.txt'),
    right: join(R, 'ws.txt'),
    options: null,
    // 真实 UI 口径：重要性经 text/open 的 rules 链下发（settings「比较规则」）
    rules: { trailingWsUnimportant: true },
  });
  ok(wsOpen?.leftLineCount === 3 && wsOpen?.rightLineCount === 3, 'text/open（尾随空白语料）行数', `left=${wsOpen?.leftLineCount}`);
  const wsSecs0 = await client.request('text/sections', { sessionId: 'e2e-text-edit-1', offset: 0, count: 100 });
  const wsChange0 = (wsSecs0?.sections ?? []).find((s) => s.kind === 'change');
  ok(wsChange0?.important === 'unimportant', '编辑前：尾随空白差异判为不重要', wsChange0?.important);

  const wsEdit = await client.request('text/edit', {
    sessionId: 'e2e-text-edit-1',
    edits: [{ side: 'left', start: 1, end: 2, text: 'beta one   ' }],
  });
  ok(Number(wsEdit?.rev) === 1 && wsEdit?.leftLineCount === 3 && wsEdit?.sectionCount === wsEdit?.sections?.length, 'text/edit 应答 rev/行数/段数', `rev=${wsEdit?.rev}`);
  const wsChange1 = (wsEdit?.sections ?? []).find((s) => s.kind === 'change');
  ok(wsChange1?.important === 'unimportant', 'R3-18 编辑后仍为不重要（引擎权威，不变红）', wsChange1?.important);

  // 反向：真实内容改动必须升为重要（恒 unimportant 的实现同样要红）
  const wsEdit2 = await client.request('text/edit', {
    sessionId: 'e2e-text-edit-1',
    edits: [{ side: 'left', start: 1, end: 2, text: 'beta TWO' }],
  });
  ok((wsEdit2?.sections ?? []).find((s) => s.kind === 'change')?.important === 'important', 'R3-18 真实内容改动升为重要');

  // 批次原子性：任一 op 越界 → 整批不落地（行表不得半应用）
  const wsBefore = await client.request('text/lines', { sessionId: 'e2e-text-edit-1', side: 'left', start: 0, count: 100 });
  let wsRejected = false;
  try {
    await client.request('text/edit', {
      sessionId: 'e2e-text-edit-1',
      edits: [
        { side: 'left', start: 0, end: 1, text: 'X' },
        { side: 'left', start: 99, end: 100, text: 'Y' },
      ],
    });
  } catch {
    wsRejected = true;
  }
  const wsAfter = await client.request('text/lines', { sessionId: 'e2e-text-edit-1', side: 'left', start: 0, count: 100 });
  ok(
    wsRejected && JSON.stringify(wsBefore?.lines) === JSON.stringify(wsAfter?.lines),
    'R3-18 越界批次原子拒绝（行表未半应用）',
    `rejected=${wsRejected}`
  );

  // 3c. R3-04：text/save 编辑落盘 —— 真实文件字节级校验（行尾/BOM/备份/另存为）
  ok(
    init?.capabilities?.textSave === true,
    "R3-04 能力位 textSave=true",
    String(init?.capabilities?.textSave),
  );
  const SV = join(dir, "save");
  mkdirSync(SV, { recursive: true });
  const svL = join(SV, "l.txt");
  const svR = join(SV, "r.txt");
  const svAs = join(SV, "as.txt");
  const origL = "alpha\r\nbeta\r\ngamma"; // CRLF + 末行无换行
  const origR = "\uFEFFone\ntwo\n"; // UTF-8 BOM + LF
  writeFileSync(svL, origL);
  writeFileSync(svR, origR);
  await client.request("text/open", {
    sessionId: "e2e-save-1",
    left: svL,
    right: svR,
    options: null,
  });
  const svEdit = await client.request("text/edit", {
    sessionId: "e2e-save-1",
    edits: [{ side: "left", start: 1, end: 2, text: "BETA" }],
  });
  const svSave = await client.request("text/save", { sessionId: "e2e-save-1" });
  const svEntry = (svSave?.saved ?? [])[0];
  ok(
    svSave?.saved?.length === 1 &&
      svEntry?.side === "left" &&
      svEntry?.path === svL,
    "R3-04 默认 both 只落盘有编辑的一侧",
    JSON.stringify(svSave?.saved?.map((e) => [e.side, e.path])),
  );
  const afterL = readFileSync(svL, "utf8");
  ok(
    afterL === "alpha\r\nBETA\r\ngamma",
    "R3-04 保留 CRLF 与末行无换行",
    JSON.stringify(afterL),
  );
  ok(readFileSync(svR, "utf8") === origR, "R3-04 未编辑侧文件不被触碰");
  ok(
    svEntry?.backupPath === `${svL}.bak` &&
      readFileSync(`${svL}.bak`, "utf8") === origL,
    "R3-04 默认备份为覆盖前内容",
    String(svEntry?.backupPath),
  );
  ok(
    svEntry?.bytes === Buffer.byteLength(afterL, "utf8") &&
      svEntry?.lineCount === 3,
    "R3-04 回执字节数/行数与落盘一致",
    `bytes=${svEntry?.bytes} lines=${svEntry?.lineCount}`,
  );
  ok(
    Number(svSave?.rev) === Number(svEdit?.rev) &&
      svSave?.leftModified === false &&
      svSave?.rightModified === false,
    "R3-04 保存不推进 rev 且清除脏标记",
    `rev=${svSave?.rev} left=${svSave?.leftModified}`,
  );
  const svNoop = await client.request("text/save", { sessionId: "e2e-save-1" });
  ok(
    svNoop?.saved?.length === 0,
    "R3-04 无编辑时保存为空操作",
    String(svNoop?.saved?.length),
  );

  // 另存为：原文件不动，且会话改指新路径（下一次默认保存不得回写原文件）
  await client.request("text/edit", {
    sessionId: "e2e-save-1",
    edits: [{ side: "left", start: 2, end: 3, text: "GAMMA" }],
  });
  const svAsRes = await client.request("text/save", {
    sessionId: "e2e-save-1",
    side: "left",
    leftPath: svAs,
  });
  ok(
    svAsRes?.saved?.[0]?.path === svAs &&
      readFileSync(svAs, "utf8") === "alpha\r\nBETA\r\nGAMMA" &&
      readFileSync(svL, "utf8") === afterL,
    "R3-04 另存为写新路径且原文件不变",
  );
  await client.request("text/edit", {
    sessionId: "e2e-save-1",
    edits: [{ side: "left", start: 0, end: 1, text: "ALPHA" }],
  });
  const svAgain = await client.request("text/save", {
    sessionId: "e2e-save-1",
  });
  ok(
    svAgain?.saved?.[0]?.path === svAs && readFileSync(svL, "utf8") === afterL,
    "R3-04 另存为后默认保存改指新路径（不回写原文件）",
    String(svAgain?.saved?.[0]?.path),
  );

  // 右侧：显式 side + backup:false → BOM 原样写回、无备份文件
  await client.request("text/edit", {
    sessionId: "e2e-save-1",
    edits: [{ side: "right", start: 0, end: 1, text: "ONE" }],
  });
  const svRRes = await client.request("text/save", {
    sessionId: "e2e-save-1",
    side: "right",
    backup: false,
  });
  ok(
    readFileSync(svR, "utf8") === "\uFEFFONE\ntwo\n" &&
      !existsSync(`${svR}.bak`) &&
      !svRRes?.saved?.[0]?.backupPath,
    "R3-04 BOM 原样写回 + backup:false 不产生备份",
    JSON.stringify(readFileSync(svR, "utf8")),
  );

  // 4. P7：三向文本合并四方法（text/merge3/open|sections|decide|output）
  const m3 = await client.request('text/merge3/open', {
    sessionId: 'e2e-merge3-1',
    base: join(dir, 'base.txt'),
    left: join(dir, 'left.txt'),
    right: join(dir, 'right.txt'),
    options: null,
  });
  ok(
    m3?.baseLineCount === 5 && m3?.leftLineCount === 5 && m3?.rightLineCount === 6 && m3?.conflictCount === 2,
    'text/merge3/open 三方行数与冲突数',
    `base=${m3?.baseLineCount} left=${m3?.leftLineCount} right=${m3?.rightLineCount} conflicts=${m3?.conflictCount}`
  );

  const m3secs = await client.request('text/merge3/sections', {
    sessionId: 'e2e-merge3-1', offset: 0, count: 100,
  });
  const m3chunks = m3secs?.chunks ?? [];
  ok(
    m3secs?.total === m3?.chunkCount && m3secs?.hasMore === false && m3chunks.length === m3?.chunkCount,
    'text/merge3/sections 块清单收齐',
    `total=${m3secs?.total} chunks=${m3chunks.length}`
  );
  // 空区间的行内容字段由引擎省略（skip_serializing_if），UI 侧按空数组兜底
  const linesOk = (v) => v === undefined || Array.isArray(v);
  ok(
    m3chunks.length > 0 && m3chunks.every((c) =>
      c.base && c.left && c.right &&
      linesOk(c.baseLines) && linesOk(c.leftLines) && linesOk(c.rightLines) &&
      Array.isArray(c.outputLines) && typeof c.unresolved === 'boolean'
    ) && m3chunks.some((c) => Array.isArray(c.baseLines)),
    'merge3 块形态同形（三区间 + 行内容可缺省 + 决策状态 + 输出行）'
  );
  const unresolved0 = m3chunks.filter((c) => c.unresolved).map((c) => c.id);
  ok(unresolved0.length === 2 && m3chunks.some((c) => c.origin === 'conflict'), 'merge3 冲突块与未决清单');

  const marker = await client.request('text/merge3/output', {
    sessionId: 'e2e-merge3-1', mode: 'marker',
  });
  ok(
    marker?.hasConflicts === true && (marker.text.match(/<<<<<<</g) ?? []).length === 2,
    'text/merge3/output marker 渲染未决冲突',
    `hasConflicts=${marker?.hasConflicts}`
  );

  const decisions = [
    { chunkId: unresolved0[0], action: 'left' },
    { chunkId: unresolved0[1], action: 'right' },
  ];
  const decided = await client.request('text/merge3/decide', {
    sessionId: 'e2e-merge3-1', decisions,
  });
  ok(
    Number(decided?.rev) === 1 && decided?.unresolvedChunks?.length === 0,
    'text/merge3/decide 决策清空未决冲突',
    `rev=${decided?.rev}`
  );
  const again = await client.request('text/merge3/decide', {
    sessionId: 'e2e-merge3-1', decisions,
  });
  ok(Number(again?.rev) === Number(decided?.rev), 'text/merge3/decide 幂等（重复提交 rev 不变）');

  const plain = await client.request('text/merge3/output', {
    sessionId: 'e2e-merge3-1', mode: 'plain',
  });
  ok(
    plain?.hasConflicts === false &&
      plain.text.includes('L1') && plain.text.includes('R2') && !plain.text.includes('<<<<<<<'),
    'text/merge3/output plain 合成决策结果',
    `hasConflicts=${plain?.hasConflicts}`
  );

  // R3-20：合并输出落盘（text/merge3/save）——真实写文件 + 备份 + rev 不递增
  ok(init?.capabilities?.mergeSave === true, 'initialize 能力位 mergeSave=true（R3-20）');
  const m3Target = join(dir, 'merged-out.txt');
  writeFileSync(m3Target, 'PREVIOUS CONTENT\n');
  const m3save = await client.request('text/merge3/save', {
    sessionId: 'e2e-merge3-1', path: m3Target, mode: 'plain',
  });
  const m3written = readFileSync(m3Target, 'utf8');
  ok(
    m3save?.path === m3Target && m3written === plain.text &&
      Number(m3save?.bytes) === Buffer.byteLength(m3written) &&
      m3save?.lineCount === plain.text.split('\n').length - 1,
    'text/merge3/save 写回合成输出（字节数/行数一致）',
    `bytes=${m3save?.bytes} lineCount=${m3save?.lineCount}`
  );
  ok(
    m3save?.backupPath === `${m3Target}.bak` &&
      readFileSync(`${m3Target}.bak`, 'utf8') === 'PREVIOUS CONTENT\n',
    'text/merge3/save 覆盖前备份原内容（.bak）'
  );
  ok(
    Number(m3save?.rev) === Number(again?.rev) && m3save?.hasConflicts === false &&
      m3save?.unresolvedChunks?.length === 0,
    'text/merge3/save 不改决策（rev 不递增）且回报未决清单',
    `rev=${m3save?.rev}`
  );
  const afterSave = await client.request('text/merge3/decide', {
    sessionId: 'e2e-merge3-1', decisions,
  });
  ok(Number(afterSave?.rev) === Number(m3save?.rev), 'text/merge3/save 后幂等 decide 不递增 rev');

  // backup:false（BC /nobackups）→ 不产生新备份，原 .bak 保持不变
  writeFileSync(m3Target, 'SECOND\n');
  const noBak = await client.request('text/merge3/save', {
    sessionId: 'e2e-merge3-1', path: m3Target, backup: false,
  });
  ok(
    noBak?.backupPath === undefined && readFileSync(`${m3Target}.bak`, 'utf8') === 'PREVIOUS CONTENT\n',
    'text/merge3/save backup=false 不写备份'
  );
  const fresh = join(dir, 'fresh-out.txt');
  const freshSave = await client.request('text/merge3/save', {
    sessionId: 'e2e-merge3-1', path: fresh,
  });
  ok(freshSave?.backupPath === undefined && existsSync(fresh), 'text/merge3/save 新目标不产生备份');
  let emptyErr = null;
  try {
    await client.request('text/merge3/save', { sessionId: 'e2e-merge3-1', path: '' });
  } catch (e) {
    emptyErr = e.message;
  }
  ok(emptyErr !== null && emptyErr.includes('-32602'), 'text/merge3/save 空路径被拒（INVALID_PARAMS）', emptyErr ?? '');

  // marker 模式 + 未决冲突：落盘文件含冲突标记，应答回报未决清单（新会话，未决策）
  await client.request('text/merge3/open', {
    sessionId: 'e2e-merge3-2',
    base: join(dir, 'base.txt'), left: join(dir, 'left.txt'), right: join(dir, 'right.txt'),
    options: null,
  });
  const markerTarget = join(dir, 'marker-out.txt');
  const markerSave = await client.request('text/merge3/save', {
    sessionId: 'e2e-merge3-2', path: markerTarget, mode: 'marker', backup: false,
  });
  const markerWritten = readFileSync(markerTarget, 'utf8');
  ok(
    markerSave?.hasConflicts === true && markerSave?.unresolvedChunks?.length === 2 &&
      (markerWritten.match(/<<<<<<</g) ?? []).length === 2,
    'text/merge3/save marker 模式落盘未决冲突并回报清单',
    `unresolved=${JSON.stringify(markerSave?.unresolvedChunks)}`
  );
  await client.request('session/close', { sessionId: 'e2e-merge3-2' });

  // 5. R3-07：文件夹三向合并（dir/mergeScan|mergePlan|mergeExecute，真实落盘）
  ok(init?.capabilities?.dirMerge === true, 'initialize 能力位 dirMerge=true');
  const dm = await client.request('dir/mergeScan', {
    sessionId: 'e2e-dmerge-1', base: MB, left: ML, right: MR,
  });
  const dmRows = new Map((dm?.rows ?? []).map((r) => [r.relPath, r]));
  const cat = (p) => dmRows.get(p)?.category;
  ok(
    cat('same.txt') === 'same' && cat('lchg.txt') === 'leftChanged' && cat('rchg.txt') === 'rightChanged' &&
      cat('conf.txt') === 'conflict' && cat('lonly.txt') === 'leftOnly' && cat('del.txt') === 'deletedRight',
    'dir/mergeScan diff3 分类',
    `rows=${dmRows.size}`
  );
  ok(
    dm?.stats?.total === dmRows.size && dm?.stats?.conflict >= 1 && dm?.stats?.orphan >= 1,
    'dir/mergeScan stats 与行数一致',
    JSON.stringify(dm?.stats)
  );
  ok(dmRows.get('conf.txt')?.conflictLeaf === true, '冲突文件行标记 conflictLeaf（双击进文本合并）');

  const dmPlan = await client.request('dir/mergePlan', {
    sessionId: 'e2e-dmerge-1', target: 'right', decisions: {},
  });
  const dmOps = dmPlan?.ops ?? [];
  ok(
    dmOps.some((o) => o.path === 'lchg.txt' && o.kind === 'takeLeft') &&
      dmOps.some((o) => o.path === 'lonly.txt' && o.kind === 'takeLeft') &&
      !dmOps.some((o) => o.path === 'conf.txt') && !dmOps.some((o) => o.path === 'same.txt'),
    'dir/mergePlan 目标右：仅左改/左孤立入计划，冲突与一致不入',
    dmOps.map((o) => `${o.kind}:${o.path}`).join(',')
  );
  const dmExec = await client.request('dir/mergeExecute', {
    sessionId: 'e2e-dmerge-1', target: 'right', ops: dmOps,
  });
  ok(
    dmExec?.failed === 0 && dmExec?.copied === dmOps.length && dmExec?.cancelled === false,
    'dir/mergeExecute 目标右全部成功',
    `copied=${dmExec?.copied} failed=${dmExec?.failed}`
  );
  ok(
    readFileSync(join(MR, 'lchg.txt'), 'utf8') === 'new\n' && readFileSync(join(MR, 'lonly.txt'), 'utf8') === 'L\n',
    '合并结果真实落盘（右侧被覆盖/新增）'
  );
  ok(
    Array.isArray(dmExec?.details) && dmExec.details.length === dmOps.length &&
      dmExec.details.every((d) => d.status === 'copied') && Array.isArray(dmExec?.log) && dmExec.log.length > 0,
    'dir/mergeExecute 明细与日志齐备'
  );

  // 输出目标：缺 outputPath 必须拒绝；给了则新建第三目录并落盘
  let dmNoOut = '';
  try {
    await client.request('dir/mergePlan', { sessionId: 'e2e-dmerge-1', target: 'output', decisions: {} });
  } catch (e) {
    dmNoOut = String(e?.message ?? e);
  }
  ok(dmNoOut.includes('-32602'), 'target=output 缺 outputPath 报 INVALID_PARAMS', dmNoOut);

  const OUT = join(dir, 'OUT'); // 故意不预建：全新第三目录是 BC 常规流程
  const outPlan = await client.request('dir/mergePlan', {
    sessionId: 'e2e-dmerge-1', target: 'output', decisions: {}, outputPath: OUT,
  });
  ok(
    (outPlan?.ops ?? []).some((o) => o.path === 'same.txt' && o.kind === 'takeLeft') &&
      !(outPlan?.ops ?? []).some((o) => o.path === 'conf.txt'),
    '输出目标计划：三方一致也复制、冲突不进输出',
    `ops=${outPlan?.ops?.length}`
  );
  const outExec = await client.request('dir/mergeExecute', {
    sessionId: 'e2e-dmerge-1', target: 'output', ops: outPlan.ops,
  });
  ok(outExec?.failed === 0 && outExec?.copied > 0, '输出目标执行成功（引擎自建第三目录）', `copied=${outExec?.copied} failed=${outExec?.failed}`);
  ok(
    existsSync(join(OUT, 'same.txt')) && readFileSync(join(OUT, 'same.txt'), 'utf8') === 's\n' &&
      readFileSync(join(OUT, 'lchg.txt'), 'utf8') === 'new\n' && !existsSync(join(OUT, 'conf.txt')),
    '第三目录内容正确（一致/左改落地，冲突缺席）'
  );
  // execute 省略 outputPath：复用 plan 记入会话的输出根
  const outExec2 = await client.request('dir/mergeExecute', {
    sessionId: 'e2e-dmerge-1', target: 'output', ops: outPlan.ops,
  });
  ok(outExec2?.failed === 0, 'execute 省略 outputPath 复用会话输出根');

  // 6. P11：report/generate（FR-RPT-001/002；真实引擎布局族 + 分块流）
  const rpt = await client.request('report/generate', {
    sessionId: 'e2e-text-1', layout: 'summary', title: 'e2e report',
  });
  ok(
    rpt?.layout === 'summary' && typeof rpt.content === 'string' && rpt.content.length > 0 &&
      rpt.totalChars > 0 && rpt.hasMore === false,
    'report/generate summary 全量报告',
    `chars=${rpt?.totalChars}`
  );
  const rptPatch = await client.request('report/generate', {
    sessionId: 'e2e-text-1', layout: 'patch',
  });
  ok(rptPatch?.content?.startsWith('--- '), 'report/generate patch 布局');

  const rptFirst = await client.request('report/generate', {
    sessionId: 'e2e-text-1', layout: 'xml', offset: 0, limit: 10,
  });
  ok(
    rptFirst?.hasMore === true && Array.from(rptFirst.content).length === 10,
    'report/generate 分块首块',
    `hasMore=${rptFirst?.hasMore}`
  );
  let rptAssembled = rptFirst.content;
  let rptOff = 10;
  while (rptOff < rptFirst.totalChars) {
    const blk = await client.request('report/generate', {
      sessionId: 'e2e-text-1', layout: 'xml', offset: rptOff, limit: 7,
    });
    rptAssembled += blk.content;
    rptOff += Array.from(blk.content).length;
    if (!blk.hasMore || blk.content.length === 0) break;
  }
  const rptFull = await client.request('report/generate', {
    sessionId: 'e2e-text-1', layout: 'xml', offset: 0, limit: rptFirst.totalChars,
  });
  ok(rptAssembled === rptFull.content, 'report/generate 分块拼接 == 全量');
  // 本客户端把 JSON-RPC error 包装成 Error("RPC <code>: <message>")
  let rptGhostMsg = '';
  try {
    await client.request('report/generate', { sessionId: 'ghost', layout: 'summary' });
  } catch (e) {
    rptGhostMsg = String(e?.message ?? e);
  }
  ok(rptGhostMsg.includes('410'), 'report/generate 未知会话报 SESSION_NOT_FOUND(410)', rptGhostMsg);

  // 6.5 R3-10：table/hex 重复 open（切回标签 / StrictMode 双挂载）→ 411 → close → reopen
  // 引擎只有一张 sessions 表，故同句柄二次 open 必被拒；UI 的幂等封装靠 close→reopen 恢复。
  const tblL = join(dir, 'a.csv');
  const tblR = join(dir, 'b.csv');
  writeFileSync(tblL, 'id,name,qty\n1,a,10\n2,b,20\n');
  writeFileSync(tblR, 'id,name,qty\n1,a,10\n2,B,25\n3,c,30\n');
  const binL = join(dir, 'a.bin');
  const binR = join(dir, 'b.bin');
  writeFileSync(binL, Buffer.from([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]));
  writeFileSync(binR, Buffer.from([0, 1, 2, 99, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]));
  const rpcCode = async (method, params) => {
    try {
      await client.request(method, params);
      return 'resolved';
    } catch (e) {
      return String(e?.message ?? e);
    }
  };

  const tblArgs = { sessionId: 'e2e-table-1', left: tblL, right: tblR, options: null };
  const tblFirst = await client.request('table/open', tblArgs);
  ok(tblFirst.rowCount > 0, 'table/open 首次成功', `rowCount=${tblFirst.rowCount}`);
  ok((await rpcCode('table/open', tblArgs)).includes('411'), 'table/open 同句柄二次 open 报 411');
  ok((await client.request('session/close', { sessionId: 'e2e-table-1' })) === null, 'table 会话 close 幂等');
  const tblAgain = await client.request('table/open', tblArgs);
  ok(tblAgain.rowCount === tblFirst.rowCount, 'close→reopen 后 table 统计一致');
  const tblSecs = await client.request('table/sections', { sessionId: 'e2e-table-1', offset: 0, count: 10 });
  ok(tblSecs.rows.length > 0, 'reopen 后 table/sections 可取数据', `rows=${tblSecs.rows.length}`);

  const hexArgs = { sessionId: 'e2e-hex-1', left: binL, right: binR, options: null };
  const hexFirst = await client.request('hex/open', hexArgs);
  ok(hexFirst.leftSize === 17 && hexFirst.rightSize === 16, 'hex/open 首次成功', `${hexFirst.leftSize}/${hexFirst.rightSize}B`);
  ok((await rpcCode('hex/open', hexArgs)).includes('411'), 'hex/open 同句柄二次 open 报 411');
  await client.request('session/close', { sessionId: 'e2e-hex-1' });
  const hexAgain = await client.request('hex/open', hexArgs);
  ok(hexAgain.leftSize === hexFirst.leftSize, 'close→reopen 后 hex 尺寸一致');
  const hexPage = await client.request('hex/bytes', { sessionId: 'e2e-hex-1', side: 'left', offset: 0, length: 16 });
  ok(hexPage.lines.length === 1, 'reopen 后 hex/bytes 可取数据', `lines=${hexPage.lines.length}`);

  // 跨种类占用同一句柄同样 411（证明 sessions 命名空间全局唯一）
  ok((await rpcCode('hex/open', { ...hexArgs, sessionId: 'e2e-table-1' })).includes('411'), '句柄跨种类冲突也报 411');
  await client.request('session/close', { sessionId: 'e2e-table-1' });
  await client.request('session/close', { sessionId: 'e2e-hex-1' });

  // 6.6 R3-14：真实引擎的 zip:// 路由（此前 HandlerCtx 固定 LocalFs，桌面端 zip 不可用）
  const zipPath = join(dir, 'repo.zip');
  writeFileSync(zipPath, buildStoredZip([['a.txt', Buffer.from('alpha\nbeta\n')]]));
  const zipLocal = join(dir, 'zip-local.txt');
  writeFileSync(zipLocal, 'alpha\nCHANGED\n');
  const zipUri = `zip://${zipPath}!/a.txt`;
  await client.request('text/open', { sessionId: 'e2e-zip-1', left: zipUri, right: zipLocal });
  const zipLines = await client.request('text/lines', {
    sessionId: 'e2e-zip-1',
    side: 'left',
    start: 0,
    count: 10,
  });
  ok(
    JSON.stringify(zipLines.lines) === JSON.stringify(['alpha', 'beta']),
    'text/open 读到了 zip 内条目',
    JSON.stringify(zipLines.lines),
  );
  const zipRight = await client.request('text/lines', {
    sessionId: 'e2e-zip-1',
    side: 'right',
    start: 0,
    count: 10,
  });
  ok(zipRight.lines[1] === 'CHANGED', 'zip 侧与本地侧混合会话成立');
  await client.request('session/close', { sessionId: 'e2e-zip-1' });

  const zipSrcRoot = join(dir, 'ZipSrc');
  mkdirSync(zipSrcRoot, { recursive: true });
  writeFileSync(join(zipSrcRoot, 'new.txt'), 'FROM-LOCAL');
  await client.request('dir/open', {
    sessionId: 'e2e-zip-2',
    left: zipSrcRoot,
    right: `zip://${zipPath}!/`,
    depth: 1,
  });
  const zipExec = await client.request('dir/execute', {
    sessionId: 'e2e-zip-2',
    ops: [{ kind: 'copy', path: 'new.txt', side: 'left' }],
  });
  ok(zipExec.failed.length === 0, '本地→zip 复制未被能力位拒绝', JSON.stringify(zipExec.failed));
  const zipEntries = readStoredZipNames(readFileSync(zipPath));
  ok(zipEntries.includes('new.txt'), '复制真的写回了归档', zipEntries.join(','));
  await client.request('session/close', { sessionId: 'e2e-zip-2' });

  // 6.7 R3-15：编码自动检测 + 显式编码覆盖 + 按原编码回写（FR-TXT-013）
  // 夹具是真实 GB18030 字节（不是 UTF-8 字符串写盘）：中D6D0 文CEC4 第B5DA 二B6FE 行D0D0 改B8C4
  const gbkBytes = Buffer.from([0xd6, 0xd0, 0xce, 0xc4, 0x0a, 0xb5, 0xda, 0xb6, 0xfe, 0xd0, 0xd0, 0x0a]);
  // 编辑只替换第 0 行，第 1 行"第二行"原样保留
  const gbkEdited = Buffer.from([
    0xd6, 0xd0, 0xce, 0xc4, 0xb8, 0xc4, 0x0a, 0xb5, 0xda, 0xb6, 0xfe, 0xd0, 0xd0, 0x0a,
  ]);
  ok(new TextDecoder('gb18030').decode(gbkBytes) === '中文\n第二行\n', 'GB18030 夹具自校验');
  ok(new TextDecoder('gb18030').decode(gbkEdited) === '中文改\n第二行\n', 'GB18030 编辑后夹具自校验');
  const encL = join(dir, 'enc-l.txt');
  const encR = join(dir, 'enc-r.txt');
  writeFileSync(encL, gbkBytes);
  writeFileSync(encR, gbkBytes);
  const encOpened = await client.request('text/open', {
    sessionId: 'e2e-enc-1',
    left: encL,
    right: encR,
  });
  ok(encOpened.leftEncoding !== 'UTF-8', '非 UTF-8 文件自动检测出编码', encOpened.leftEncoding);
  const encLines = await client.request('text/lines', {
    sessionId: 'e2e-enc-1',
    side: 'left',
    start: 0,
    count: 10,
  });
  ok(encLines.lines[0] === '中文', 'GB18030 内容正确解码', JSON.stringify(encLines.lines));
  await client.request('text/edit', {
    sessionId: 'e2e-enc-1',
    edits: [{ side: 'left', start: 0, end: 1, text: '中文改' }],
  });
  await client.request('text/save', { sessionId: 'e2e-enc-1' });
  const encSaved = readFileSync(encL);
  ok(encSaved.equals(gbkEdited), '编辑后仍按 GB18030 回写（未被改写成 UTF-8）', encSaved.toString('hex'));
  ok(readFileSync(encR).equals(gbkBytes), '未编辑侧字节不动');
  await client.request('session/close', { sessionId: 'e2e-enc-1' });

  // 显式编码压过自动检测：同一份 UTF-8 字节按 windows-1252 解出 mojibake
  const utf8L = join(dir, 'enc-utf8.txt');
  writeFileSync(utf8L, 'café\n');
  const encExplicit = await client.request('text/open', {
    sessionId: 'e2e-enc-2',
    left: utf8L,
    right: utf8L,
    encoding: 'windows-1252',
  });
  ok(encExplicit.leftEncoding === 'windows-1252', '显式编码生效', encExplicit.leftEncoding);
  const encMojibake = await client.request('text/lines', {
    sessionId: 'e2e-enc-2',
    side: 'left',
    start: 0,
    count: 1,
  });
  ok(encMojibake.lines[0] === 'cafÃ©', '按所选编码解码（不是自动检测）', encMojibake.lines[0]);
  await client.request('session/close', { sessionId: 'e2e-enc-2' });
  ok(
    (await rpcCode('text/open', { sessionId: 'e2e-enc-3', left: utf8L, right: utf8L, encoding: 'no-such' })).includes('413'),
    '未知编码标签报 413'
  );

  // 7. R3-03：异名手动对齐 → 同步计划 → 正确落盘（工作包 1A 真实引擎验收）
  {
    const AL = join(dir, 'AL');
    const AR = join(dir, 'AR');
    mkdirSync(AL);
    mkdirSync(AR);
    writeFileSync(join(AL, 'a.txt'), 'AAA\n');
    writeFileSync(join(AR, 'b.txt'), 'BBB\n');

    const alignOpen = await client.request('dir/open', {
      sessionId: 'e2e-align-1',
      left: AL,
      right: AR,
      depth: 1,
      criteria: { mtimeToleranceSec: 0, contentCompare: 'always' },
      manualAlign: [{ left: 'a.txt', right: 'b.txt' }],
    });
    ok(alignOpen?.rowCount === 1, '预置 manualAlign 后只剩一行（不再拆成两行）', `rowCount=${alignOpen?.rowCount}`);

    const alignPlan = await client.request('dir/operate', {
      sessionId: 'e2e-align-1',
      preset: 'mirrorLeftToRight',
    });
    const alignOps = alignPlan?.plan ?? [];
    ok(alignOps.length === 1, '异名配对只产生一个操作', JSON.stringify(alignOps));
    ok(
      alignOps[0]?.sourcePath === 'a.txt' && alignOps[0]?.targetPath === 'b.txt',
      '复制操作的源/目标分别来自左右真实路径',
      JSON.stringify(alignOps[0]),
    );

    const alignExec = await client.request('dir/execute', {
      sessionId: 'e2e-align-1',
      ops: [{ kind: 'copy', path: 'a.txt', side: 'left', targetSide: 'right', targetPath: 'b.txt' }],
    });
    ok((alignExec?.failed?.length ?? 0) === 0, '异名复制执行无失败', JSON.stringify(alignExec?.failed));
    ok(
      readFileSync(join(AR, 'b.txt'), 'utf8') === 'AAA\n',
      '右侧真实文件被写成左侧内容',
      JSON.stringify(readFileSync(join(AR, 'b.txt'), 'utf8')),
    );
    ok(!existsSync(join(AR, 'a.txt')), '不得按源名在右侧另建一份');
    ok(readFileSync(join(AL, 'a.txt'), 'utf8') === 'AAA\n', '源文件未被改写');
    await client.request('session/close', { sessionId: 'e2e-align-1' });
  }

  // 7b. D0：从**右侧**路径解除手动对齐（左右入口必须对称）→ 真实落盘
  {
    const UL = join(dir, 'UL');
    const UR = join(dir, 'UR');
    mkdirSync(UL);
    mkdirSync(UR);
    writeFileSync(join(UL, 'a.txt'), 'AAA\n');
    writeFileSync(join(UR, 'b.txt'), 'BBB\n');

    await client.request('dir/open', {
      sessionId: 'e2e-unalign-1',
      left: UL,
      right: UR,
      depth: 1,
      // 对齐/解除对齐需要 P2 会话模型
      lazyScan: true,
      criteria: { mtimeToleranceSec: 0, contentCompare: 'always' },
    });
    const aligned = await client.request('dir/align', {
      sessionId: 'e2e-unalign-1',
      left: 'a.txt',
      right: 'b.txt',
    });
    ok(aligned?.updatedRows === 2, '异名对齐：两行受影响', JSON.stringify(aligned?.updatedRows));

    const un = await client.request('dir/align', { sessionId: 'e2e-unalign-1', unalign: 'b.txt' });
    ok(
      un?.stats?.total === 2 && un.stats.leftOnly === 1 && un.stats.rightOnly === 1,
      '右侧入口解除后恢复为两个孤立项（leftOnly=1 + rightOnly=1）',
      JSON.stringify(un?.stats),
    );

    const unPlan = await client.request('dir/operate', {
      sessionId: 'e2e-unalign-1',
      preset: 'mirrorLeftToRight',
    });
    const unOps = unPlan?.plan ?? [];
    ok(unOps.length === 2, '解除后镜像计划恢复为「复制 + 删除」', JSON.stringify(unOps));
    const copyOp = unOps.find((o) => o.op === 'copyLeftToRight');
    const delOp = unOps.find((o) => o.op === 'deleteRight');
    ok(
      copyOp?.sourcePath === 'a.txt' && copyOp?.targetPath === 'a.txt',
      '左孤立项按源名复制到对侧',
      JSON.stringify(copyOp),
    );
    ok(delOp?.operationPath === 'b.txt', '右孤立项删除取右侧真实路径', JSON.stringify(delOp));

    const unExec = await client.request('dir/execute', {
      sessionId: 'e2e-unalign-1',
      ops: [
        { kind: 'copy', path: 'a.txt', side: 'left', targetSide: 'right' },
        { kind: 'delete', path: 'b.txt', side: 'right' },
      ],
    });
    ok((unExec?.failed?.length ?? 0) === 0, '解除后执行无失败', JSON.stringify(unExec?.failed));
    ok(readFileSync(join(UR, 'a.txt'), 'utf8') === 'AAA\n', '左孤立项已复制到右侧同名项');
    ok(!existsSync(join(UR, 'b.txt')), '右孤立项已被镜像删除');
    ok(readFileSync(join(UL, 'a.txt'), 'utf8') === 'AAA\n', '源侧未被改写');
    await client.request('session/close', { sessionId: 'e2e-unalign-1' });
  }

  // 8. 会话清理 + shutdown
  ok((await client.request('session/close', { sessionId: 'e2e-dir-1' })) === null, 'session/close dir 会话');
  ok((await client.request('session/close', { sessionId: 'e2e-text-1' })) === null, 'session/close text 会话');
  ok((await client.request('session/close', { sessionId: 'e2e-merge3-1' })) === null, 'session/close merge3 会话');
  ok((await client.request('session/close', { sessionId: 'e2e-dmerge-1' })) === null, 'session/close 文件夹合并会话');
  ok((await client.request('shutdown', null)) === null, 'shutdown 应答 null');
  const exitCode = await client.close();
  ok(exitCode === 0, '引擎进程随 shutdown 退出', `exit=${exitCode}`);
} catch (e) {
  ok(false, `异常中断: ${e.message}`);
  await client.close();
} finally {
  rmSync(dir, { recursive: true, force: true });
}

console.log(failed === 0 ? '\n✅ 真实引擎端到端：全部通过' : `\n❌ 端到端：${failed} 项失败`);
process.exit(failed === 0 ? 0 : 1);
