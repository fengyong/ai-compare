#!/usr/bin/env node
/**
 * 真实引擎端到端验收：ui ⇄ ai-engine 的 M1 walking-skeleton 闭环。
 *
 * 直接 spawn `../target/debug/ai-engine --stdio`（workspace 根 target），
 * 走完整协议链路（docs/03 §2.3）：
 *   initialize → dir/open → dir/delta 收批 → 状态断言
 *   → text/open → text/sections → session/close → shutdown → 进程退出
 *
 * 用法：pnpm e2e:engine（或 node scripts/e2e-engine.mjs [引擎路径]）
 * 退出码：0 = 通过；1 = 失败。引擎未构建时给出构建指引并退出 2。
 */
import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
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

import { existsSync } from 'node:fs';

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
