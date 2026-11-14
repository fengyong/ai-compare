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

  // 7. 会话清理 + shutdown
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
