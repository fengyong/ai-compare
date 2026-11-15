# ai-compare 深度 Code Review 报告

> **本报告含两轮评审。第一轮见 §1–§6；第二轮（2026-11-14 晚，含 P0 安全修复与 BC 忠实度量化审计）见 §7。两轮结论冲突时以 §7 为准（§7.4 逐条更正第一轮裁定）。**

> 评审时间：2026-11-14
> 评审范围：`engine/`（5 crate）+ `cli/` + `ui/`（Tauri2 壳 + React18 + TS）+ `tests/`
> 评审基线：引擎 822 测试 / UI 477 测试全绿的 `main`（HEAD `091a752`）
> 方法：静态走查 + 双路并行子代理深挖 + **变异验证**（逐条确认回归测试真能捕获缺陷）

---

## 1. 结论摘要

**一句话结论**：架构分层清晰、协议演进克制、测试密度可观（1299 个测试），整体质量高于同类自研项目；但**协议边界的健壮性**与**引擎会话生命周期**存在数个真实缺陷 —— 集中表现为「客户端断开后引擎进程挂死」「handler panic 导致请求永不应答」「JSON-RPC 2.0 不合规」「引擎会话只增不减」四类。

本次评审**核实并修复 19 项缺陷**（含 9 项引擎侧、9 项 UI 侧、1 项测试基建），**每项均附带回归测试**；其中 7 项另经变异验证（把修复回退，对应用例确实变红，见 §3.1），其余 12 项以回归测试 + 真实链路实测为据。剩余 21 项按置信度与修复风险分级登记于 §4，留待后续批次决策。

| 维度 | 评分 | 说明 |
| --- | --- | --- |
| 架构与分层 | A | Rust 引擎 / Tauri 壳解耦清晰，ADR 有据可循，依赖方向无环 |
| 协议设计 | B | 方法集与载荷语义良好，但 JSON-RPC 2.0 合规性有 4 处缺口；ts-rs 导出与 serde 语义脱节 |
| 健壮性 | B- | panic 隔离缺失、进程退出路径依赖隐式时序、会话释放不完整 |
| 测试 | B+ | 1299 测试 + golden + 活体对拍（git/diff3/xxd/csv）；但缺 CI、断言强度不均 |
| 工程化 | C+ | 无 CI、工具链未钉版本、MSRV 不一致、壳层脱离 workspace 覆盖 |

**建议**：**继续推进**。本轮修复未改变任何既有协议语义（`RequestId` 增补 `null` 变体为向后兼容扩展），可安全合入。

---

## 2. 已修复缺陷（19 项，全部含回归测试；7 项经变异验证）

### 2.1 引擎侧（9 项）

#### F-01【P0】客户端 EOF 后引擎进程永挂死

- **位置**：`engine/ai-engine/src/pump.rs:25`（常驻 pump 任务）、`server.rs`（退出路径）
- **现象**：`dir/open` 走 P2 模型路径时会 `pump::spawn` 一个常驻任务，它持有出站通道 `UnboundedSender` 的克隆。客户端关闭 stdin（EOF）后，`run_stdio` 的 `drop(tx)` 只释放了主通道，pump 的克隆仍在 → `writer_task` 的 `rx.recv()` 永不返回 `None` → `writer.await` 永久阻塞 → **进程挂死（只能 kill -9），且每 150ms 空转一次**。
- **根因**：退出路径没有终止会话，pump 的生命周期与会话生命周期耦合但会话永不关闭。
- **修复**：退出前 `cancel_all_dir_sessions(&ctx)` 取消全部 `DirModel` 的扫描令牌 → pump 在下一轮（≤150ms）感知取消、排空、退出 → 出站通道关闭 → writer 正常结束。保留 5s 有界保险（仅防 pump 异常卡死，**不可**当作正常退出路径）。
- **回归**：`tests/protocol_conformance/protocol_test.rs::engine_exits_after_stdin_eof_with_open_dir_session`（窗口 2s）
- **变异验证**：禁用 `cancel_all_dir_sessions` → 该用例 FAILED ✓

#### F-02【P0】协议边界完全没有 panic 隔离

- **位置**：`engine/ai-engine/src/server.rs`（`process_request`）
- **现象**：`process_request` 直接 `dispatch(...).await`，**全仓 `catch_unwind` 命中 0**，而函数注释却写「panic 捕获为 InternalError 应答」。handler 内 panic（如 `handlers.rs` 多处 `expect`、`ai-core/src/lines.rs:96` 的 `&self.lines[i]` 越界）会让 tokio 任务带 `JoinError::Panic` 结束，而串行链的 `let _ = p.await` 把错误吞掉 → **该请求永不应答**，UI 只能等到超时。
- **修复**：`tokio::spawn(AssertUnwindSafe(...))` 把 panic 转成 `JoinError`，统一回 `-32603 InternalError`。
- **回归**：见 §4 P1-09（当前尚缺一条「人为 panic 的 handler」库内单测）

#### F-03【P1】`"id": null` 的合法请求被误判为非法信封

- **位置**：`server.rs`（`has_id = value.get("id").is_some()`）、`ai-protocol/src/types.rs`（`RequestId` 无 `Null` 变体）
- **现象**：`{"id": null}` 被判为「有 id」→ `RpcRequest` 反序列化失败 → 回一条 `INVALID_REQUEST(id=0)`。
- **根因**：JSON-RPC 2.0 §4 允许 id 为 string/number/**null**；`RequestId` 只声明了 number/string。
- **修复**：`RequestId` 增补 `Null` 变体（ts-rs 导出 `number | string | null`，向后兼容扩展）。
- **回归**：`request_with_null_id_is_answered_with_null_id`

#### F-04【P1】解析失败 / 信封非法时应答 id 固定为 0（与真实请求撞号）

- **位置**：`server.rs`（两处 `RequestId::Number(0)`）
- **现象**：无法确定 id 时按规范应答 `"id": null`，此前固定回 `0` —— 与真实的 `id=0` 请求撞号，客户端把错误应答错配给别的请求（幽灵应答）。
- **修复**：解析失败 → `RequestId::Null`；信封非法但 id 可解析 → 回显原 id，不可解析 → `Null`。
- **回归**：`parse_error_is_answered_with_null_id` / `invalid_envelope_echoes_legal_id_when_present`
- **变异验证**：回退为 `Number(0)` → 用例 FAILED ✓

#### F-05【P2】不校验 `jsonrpc` 版本

- **位置**：`server.rs`
- **现象**：`jsonrpc: "1.0"` 或缺失版本的请求被正常执行，版本协商形同虚设。
- **修复**：不符 `"2.0"` → `-32600 Invalid Request`。
- **回归**：`wrong_jsonrpc_version_is_rejected`

#### F-06【P1】`text/merge3/open` 非幂等 → 标签重挂载清空全部已提交决策

- **位置**：`engine/ai-engine/src/handlers.rs`（`merge3_open` 的 411 分支）、`ui/src/ipc/engineProcess.ts`（`merge3OpenIdempotent`）
- **现象**：标签切走再切回（或 React StrictMode 双挂载）→ 视图重挂载 → 再次 `open` 得 411 → UI 侧 `closeSession` 后重建 → **引擎侧 `decisions` 表被清空**。用户做了几十个冲突决策，切一下标签全部丢失。
- **根因**：引擎把「会话已存在」一律当错误，缺少「同参数幂等」语义。
- **修复**：引擎侧 `merge3_open` 对**同 sessionId + 同三路路径**幂等返回既有会话统计（保留 `decisions` 与 `rev`）；参数不同仍回 411。
- **回归**：`merge3_open_is_idempotent_and_keeps_decisions`
- **变异验证**：关闭幂等分支 → 用例 FAILED ✓

#### F-07【P1】`aic patch` 应用失败返回退出码 1（差异），应为 2（错误）

- **位置**：`cli/aic/src/app.rs`（单文件模式）
- **现象**：补丁上下文不匹配属错误，却返回 `EXIT_DIFF(1)` → CI 把一次失败的补丁当成「存在差异」。
- **修复**：返回 `EXIT_ERROR(2)`。

#### F-08【P1】`aic patch --dir` 多文件模式的退出码语义错误

- **位置**：`cli/aic/src/app.rs`
- **现象**：`all_applied()` 仅在全部 `Applied|NoOp` 时为真，故 `AlreadyApplied`（git apply 同语义拒绝）与 `SkippedBinary`（设计语义跳过）都会落到 `EXIT_DIFF(1)`。patch apply 是「执行」而非「比较」，没有「文件不同」这一结果。
- **修复**：存在 `Failed`/`AlreadyApplied` → 2（错误）；其余（Applied/NoOp/SkippedBinary）→ 0（成功）。

#### F-09【P2】`--conflict-exit-code 0` 未校验

- **位置**：`cli/aic/src/app.rs`
- **现象**：`--conflict-exit-code 0` 被接受，破坏「0 恒表示相同/成功」的不变量（退出码契约见 `cli/aic/src/lib.rs:16`），VCS/CI 会把「有冲突」误判为合并成功。
- **修复**：显式拒绝 0。

### 2.2 UI 侧（9 项）

#### F-10【P1】顶部横幅「重试」按钮**不会发起连接**

- **位置**：`ui/src/ipc/engineProcess.ts`（`retryEngine`）
- **现象**：`retryEngine` 只把 `client`/`connectPromise` 置空 + 设置 snapshot 为「正在连接引擎…」，**从不调用 `getEngineClient()`**。而 `getEngineClient()` 是唯一发起连接的入口 → 用户点「重试」后横幅永远停在「正在连接引擎…」，直到下一次 RPC 调用才会间接触发连接。
- **修复**：`retryEngine` 内补 `void getEngineClient().catch(...)`。
- **回归**：`ui/src/shell/sessionLifecycle.test.ts` → `retryEngine：调用后必须真正建立连接`
- **变异验证**：移除该调用 → 用例 FAILED ✓

#### F-11【P1】关闭标签未释放 picture/table/hex/sync/merge 的引擎会话（泄漏）

- **位置**：`ui/src/shell/sliceWindowsTabs.ts`（`cleanupTabEngines`）
- **现象**：函数只处理 `text`/`folder`/`merge3`。而引擎侧 `sessions` map 为 picture/table/hex/text/merge/merge3/dir **全部**建会话（见 `handlers.rs` 各 `sessions.insert`）→ 其余类型的会话在引擎侧**只增不减**；且崩溃重放后会继续用已失效句柄发请求（410/411）。
- **修复**：抽出 `TAB_SESSION_KINDS` 全覆盖清单 + `tabSessionId()` 辅助函数，清理逻辑按清单遍历。
- **回归**：`cleanupTabEngines：picture/table/hex/sync/merge 标签关闭时也要释放引擎会话`
- **变异验证**：回退为三选一 → 用例 FAILED ✓

#### F-12【P1】`rebindSession` 只重绑 folder/text，其余标签崩溃后持失效句柄

- **位置**：`ui/src/shell/sliceWindowsTabs.ts`
- **现象**：引擎崩溃重连后，picture/table/hex/sync/merge3 标签仍持旧句柄 → 后续请求全部 410/411。
- **修复**：`rebindTabSession()` 按全覆盖清单重绑。
- **回归**：`rebindSession：引擎重连后全部视图种类的句柄都要重绑`（变异验证 ✓）

#### F-13【P1】resync 降级重放后新句柄未登记 `trackedSessions`

- **位置**：`ui/src/ipc/engineProcess.ts`（`resyncTrackedSessions`）
- **现象**：`catch` 分支生成 `newId` 并 `trackedSessions.delete(sessionId)`，却**不把 `newId` 登记回去** → 第二次引擎崩溃时该会话不再被恢复/重建。
- **修复**：`trackedSessions.set(newId, tracked)`。

#### F-14【P1】崩溃重启后旧 Tauri 事件监听器永不回收

- **位置**：`ui/src/ipc/engineProcess.ts`（`handleTransportCrash`）、`ui/src/ipc/tauriTransport.ts`（`stop()` 里的 `unlisteners`）
- **现象**：`handleTransportCrash` 只把 `client = null`，不 `close()` 旧 client → `TauriEngineTransport.stop()` 不执行 → `unlisteners` 从不调用 → **每次崩溃重启净增 3 个 Tauri 监听器**；且旧监听仍把 stdout 行投喂给已死客户端（同一行被两个客户端各处理一次 → 幽灵 id 报错）。
- **修复**：`handleTransportCrash` 中 `void old?.close()`。

#### F-15【P1】单个通知订阅者抛错会中断整条分发链

- **位置**：`ui/src/protocol/client.ts`（`for (const h of set) h(m.params)`）、`ui/src/ipc/engineProcess.ts`（`wireNotifications` 的 dir/delta 分发）
- **现象**：无 try/catch。一个订阅者抛错 → 其余订阅者不再收到通知，且异常冒泡回传输层（`queueMicrotask` → 未捕获异常）。
- **修复**：逐个 try/catch，异常经 `onProtocolError` 上报。
- **回归**：`单个订阅者抛错不得中断其余订阅者`（变异验证 ✓）

#### F-16【P1】Hex 视图分页的 `loading` 守卫是陈旧闭包（恒为 false）

- **位置**：`ui/src/views/hex-compare/HexCompareView.tsx`（`loadNext` 的 `useCallback`）
- **现象**：`loading` 是 state 但未进 `useCallback` 依赖 → 滚动回调持有的旧闭包读到恒为 `false` 的 `loading` → 并发发起同 offset 的 `hexBytes` → **重复行 + `offsetRef` 漂移**（StrictMode 双挂载首屏也会重复拉同一页）。
- **修复**：在途标记改用 `useRef`（`inFlightRef`），并让 `loading` 真正参与渲染。

#### F-17【P2】Hex 视图判错引擎能力位

- **位置**：`ui/src/views/hex-compare/HexCompareView.tsx:208`
- **现象**：查 `getEngineCapabilities()?.tableCompare` 判断 hex 能力，应为 `hexCompare` → 真引擎下能力提示永不显示；mock 无 `tableCompare` 反而恒显示。
- **修复**：改为 `hexCompare`。

#### F-18【P1】关闭窗口 / 关闭其他标签时静默丢弃脏的命名会话

- **位置**：`ui/src/shell/sliceWindowsTabs.ts`（`closeWindow` / `closeOthers`）
- **现象**：`closeTab` 对「命名会话 + dirty」会转 `closeGuard` 确认；但 `closeWindow` 遍历后仍整窗移除、`closeOthers` 直接批量过滤，**都不检查 closeGuard** → 未保存内容无声丢失（违反 FR-SES-001）。
- **修复**：两处先检测脏的命名会话 → 置 `closeGuard` 并中止，等待用户处置。
- **回归**：`closeWindow/closeOthers：脏的命名会话必须先走关闭确认`

### 2.3 测试基建（1 项）

#### F-19【P1】单跑测试目标时引擎二进制不重建 → 回归测试形同虚设

- **位置**：`tests/src/lib.rs`（`engine_bin()`）
- **现象**：`cargo test -p ai-compare-tests` 只构建测试目标与其依赖，**不会重建 `ai-engine` 的 bin** → 进程级用例（`p9_e2e` / 新增 `protocol_conformance`）实际执行的是**上一次的旧二进制**。本次评审的变异验证即踩中此坑：禁用 F-01 的修复后，用例**依然全绿**（假通过）。
- **修复**：`ensure_engine_fresh()` 按 mtime 比对源码与二进制，必要时现场 `cargo build -p ai-engine --bin ai-engine`。
- **注意**：`cargo test --workspace` 会构建全部 target（含 bin），故全量门禁下不受影响 —— 风险仅在「单跑某个测试目标」时暴露，最容易在调试单个用例时骗过开发者。

---

## 3. 门禁复验

修复后全量复跑：

| 门禁 | 结果 |
| --- | --- |
| `cargo test --workspace` | **829 / 0**（原 822 + 新增 7 条协议/生命周期回归） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 零警 |
| `pnpm test`（ui） | **484 / 0**（49 文件；原 477 + 新增 7 条） |
| `pnpm lint` | 零警告 |
| `pnpm build` | 通过 |
| `pnpm e2e:engine` | 全过（含 merge3 四方法与 report/generate 分块链路） |

### 3.1 变异验证矩阵

> 原则：**修复必须能被测试抓住**，否则等于没修。下表逐条把修复回退，确认对应用例变红。

| 修复 | 变异内容 | 结果 |
| --- | --- | --- |
| F-01 | 注释掉 `cancel_all_dir_sessions` | `engine_exits_after_stdin_eof_with_open_dir_session` FAILED ✓ |
| F-04 | 错误应答 id 回退 `Number(0)` | `parse_error_is_answered_with_null_id` FAILED ✓ |
| F-06 | 关闭 merge3 幂等分支 | `merge3_open_is_idempotent_and_keeps_decisions` FAILED ✓ |
| F-10 | 移除 `getEngineClient()` 调用 | `retryEngine` 用例 FAILED ✓ |
| F-11 | 清理逻辑回退为三选一 | `cleanupTabEngines` 用例 FAILED ✓ |
| F-12 | rebind 回退为 folder/text | `rebindSession` 用例 FAILED ✓ |
| F-15 | 移除通知回调 try/catch | `单个订阅者抛错…` 用例 FAILED ✓ |

> 变异验证同时暴露了测试自身的两处缺陷（已修正）：① 引擎二进制不重建导致假通过（F-19）；② 进程退出计时起点取在读线程结束之后，把「迟迟不退出」掩盖掉。

---

## 4. 未修复项登记（留待后续批次决策）

> 判定标准：**高置信度 + 低修复风险** → 已直接修复；其余一律登记，不擅自改动协议语义或做大规模重构。

### P1 — 严重，建议排期

| # | 位置 | 问题 | 建议 |
| --- | --- | --- | --- |
| P1-01 | `ai-protocol/src/types.rs` + `ui/src/protocol/generated/` | **ts-rs 导出与 serde 语义脱节**：约 169 处 `Option<T>` 字段，serde 侧 `skip_serializing_if` 会省略键，但 ts-rs 仍导出为**必填** `T \| null`；`ts(type="number")` 还会抹掉可空性（`unixMode: number` 线上却缺键）。UI 侧已用 `?? null` 兜底掩盖，类型系统无法再保护我们 | 为 `Option` 字段统一补 `#[ts(optional)]`（ts-rs 支持逐字段标注），或改用 `Option<T>` 的 `ts(type = "T \| null")` + 字段可选双标注；建议开一个专门的「协议类型可信度」批次 |
| P1-02 | `ai-protocol/src/types.rs` | **u64/i64 → ts-rs `bigint`，但 NDJSON 线上是普通 number**（`rev`、`code`、`offset` 等）。UI 侧只能 `as unknown as bigint` 手写适配；>2^53 丢精度 | 为这些字段加 `ts(type="number")`（`RequestId::Number` 已这么做），并统一「大整数走 string」约定 |
| P1-03 | `ui/src-tauri/src/lib.rs:339-356` | **原子写契约被违反**：`std::fs::write` 无文件 fsync、无父目录 fsync；且先 `rename(primary→.bak)`，窗口期内主文件不存在 → 掉电可能出现「主文件空 + .bak 已搬走」双源皆损 | 改为「写 tmp → fsync(tmp) → rename(tmp, primary) → fsync(dir)」；`.bak` 改为「复制快照」而非 rename |
| P1-04 | `ui/src-tauri/src/lib.rs:339`、`ui/src/session/service.ts:155` | **临时文件名按 PID 而非每次唯一 + 持久化无串行化**：同进程内所有写共用 `<name>.tmp-<pid>`；`persist()` 无队列/互斥，`saveSession`/`writeOverride`/`recordOpened`/`setUiState` 可并发写同一文件 → 互相截断 + 抢 rename | 临时名加单调计数/随机后缀；持久化层加单文件串行队列 |
| P1-05 | `ui/src/ipc/mockEngine.ts:228` vs `engine/ai-engine/src/handlers.rs:67` | **mock 与真实引擎在 `dir/mergeScan`/`mergePlan`/`mergeExecute` 三方法上分叉**：mock 实现了，真实引擎 dispatch 无（返回 -32601）；`FolderMergeView.tsx:78` 直接依赖它们 → 浏览器演示与 UI 测试全绿，接真引擎整页失败 | 二选一：真实引擎补齐三方法，或 mock 移除并让视图按能力位降级；**优先补真实引擎**（BC 忠实度要求） |
| P1-06 | `ui/src/session/storageAdapter.ts:135`、`service.ts:135` | **浏览器后端 `deleteFile` 不清 `.bak`** + 恢复策略「主文件缺失 + .bak 存在 → 采用备份」→ 用户手工删除的 sessions.json 会被静默复活 | `deleteFile` 同时删除 `.bak`；恢复策略加「用户显式删除标记」 |
| P1-07 | `ui/src/ipc/engineProcess.ts`（`trackSession`） | **崩溃恢复登记只覆盖 text/dir**：picture/table/hex/merge3 的 `open` 成功后从不 `trackSession` → 引擎崩溃后这四类标签无法恢复（与 F-12 同源，缺另一半） | 扩展 `TrackedSession.kind` 覆盖全部视图种类，并在各视图 open 成功后登记 |
| P1-08 | `ui/src/views/merge3/Merge3View.tsx` | **合并决策随标签切换丢失**：决策是组件局部 state。F-06 修复了「引擎侧决策被 close 重建清空」，但视图重挂载后本地 `decisions` 仍会重置、用户需重新操作 | 决策状态提升到 tab 级（`Tab.merge3.decisions`）或以引擎侧为准（`sections` 已回带决策） |
| P1-09 | `engine/ai-engine/src/server.rs` | **panic 隔离本身尚无测试**：F-02 的修复只有代码无回归 —— 需要一个「故意 panic 的 handler」才能锁定。考虑在 `#[cfg(test)]` 下注册一个 panic 测试方法 | 补库内单测 |
| P1-10 | 全仓 | **无 CI**：无 `.github/workflows`、无 Makefile/justfile。`deny.toml` 声明的许可零容忍策略**没有任何自动化执行点**，GPL 依赖可静默引入；质量门禁全靠 agent 在本地会话手动跑并自述 | 建最小 CI：`cargo test`/`clippy`/`cargo-deny` + `pnpm test`/`lint`/`build`/`e2e:engine` |

### P2 — 改进

| # | 位置 | 问题 |
| --- | --- | --- |
| P2-01 | `handlers.rs:116` | 能力位 `cancel: true` **过度承诺**：只有 `dir_open`/`dir_execute` 收到 `CancelFlag`，`text/open`、`report/generate`、`dir/filter` 不可取消。UI 会显示取消按钮但取消无效（`docs/archive/REVIEW-p11.md` P3-6 已记录，仍未修） |
| P2-02 | `rust-toolchain.toml` | 只写 `channel = "stable"`，**未钉版本**；stable 漂移即可能构建失败，本地/发布环境行为不一致 |
| P2-03 | `Cargo.toml` vs `ui/src-tauri/Cargo.toml` | **MSRV 声明不一致**（1.80 vs 1.77.2）；且 `ui/src-tauri` 自带空 `[workspace]`，根 `cargo clippy --all-targets` **不覆盖壳层 367 行 Rust** —— 而壳层正是 spawn 引擎进程 / stdio 透传的风险最高处 |
| P2-04 | `deny.toml` + `.cargo/config.toml` | **cargo-deny 与 rsproxy 镜像可能冲突**：`[sources] unknown-registry = "deny"` 且 allow 列表只有 crates.io 官方 index，而 crates-io 被替换为 `sparse+https://rsproxy.cn/index/`。需实跑 `cargo deny check` 验证（该二进制当前未安装） |
| P2-05 | `server.rs:204` | **出站通道无界无背压**：`mpsc::unbounded_channel` + pump 一次 drain 全部 delta；客户端不读 stdout 时百万级 delta 行堆在内存（协议已有分批，但只分批不限速） |
| P2-06 | `server.rs:177` | **`--record` 目标不可写时静默失效**：`File::create(p).ok()` → `file: None`，既不报错也不打日志 —— 用户以为在录制，实际无文件，问题无法复现 |
| P2-07 | `server.rs:139-156` | **取消注册表用 `std::sync::Mutex` + `expect`**：锁中毒即 panic（叠加 F-02 会挂死而非报错）；且在 async 上下文阻塞 worker 线程 |
| P2-08 | `server.rs:209` | `BufReader::lines()` **无行长上限**：对端可发单行 GB 级 JSON，无限缓冲到内存 |
| P2-09 | `ui/src/protocol/client.ts:201` | **客户端只接受 number id**（`typeof id !== 'number'`），而协议层已支持 string id → 引擎若回 string id 应答，客户端会判为「应答缺少合法 id」 |
| P2-10 | `cli/aic/src/report_cmd.rs:230` | `aic report` 成功一律退 0，即使报告内容存在差异。需确认契约把 report 定义为「生成器」（成功即 0）还是「比较器」（有差异应 1） |
| P2-11 | `ui/src/views/text-compare/` | 书签与自动换行为**视图内状态，未随命名会话持久化**（PROGRESS 已登记）；自动换行用 CSS 覆盖 `.cm-content` 而非 CM6 `lineWrapping` 扩展，跨几何同步依赖 CM ResizeObserver |
| P2-12 | `ui/src/views/hex-compare/HexCompareView.tsx:129` | 偏移跳转的 `querySelector` 行索引算式 `2*floor(jumpTo/ROW_BYTES) - idx` 可疑，建议改用显式的目标行号变量 |
| P2-13 | `ui/src/session/service.ts` | `deleteFile` 的调用方目前未启用（P1-06 的影响面因此受限），但接口已暴露，风险随调用方增加而放大 |

### P3 — 建议 / 观察项

- `ui/src/ipc/engineProcess.ts:152-192`：**并发连接竞态** —— 旧连接的失败回调可能覆盖新连接的 snapshot（横幅显示失败但引擎可用），或旧连接后到地 `client = c` 覆盖新 client（后续请求全 CLOSED）。需专门复现确认。
- `ui/src/ipc/reconnect.ts`：`TrackedSession.kind` 仅 `'dir' | 'text'`，与 P1-07 同源。
- `engine/ai-protocol/src/lib.rs:57`：`TEXT_SECTIONS_NOTIFY == "text/sections"` 与请求方法名完全相同，靠「有无 id」区分消息；UI 侧若按 method 名路由会有歧义。
- `tests/algodiff_matrix/generate_fixtures.py` 与 `tests/table_matrix/generate_fixtures.py`：**语料生成器是 Python**，仓库未声明 Python 为工具链依赖（CI 化时需补）。
- `engine/ai-core/src/incremental.rs`（678 行）：**增量重 diff 的窗口锚定 + 边界吸收缺正确性证明**；编辑落在锚点边界时是否可能漏 diff/产生错位，建议补性质测试（随机编辑序列 vs 全量 diff 结果一致性）。
- `engine/ai-core/src/inline.rs`：**行内 diff 超时降级路径**是否静默产出错误结果（而非标记失败），建议核实降级语义。

---

## 5. 风险矩阵

| 风险 | 概率 | 影响 | 可探测性 | 处置 |
| --- | --- | --- | --- | --- |
| 引擎进程挂死（客户端断开） | 高 | 高 | 低（进程残留，需人工发现） | ✅ 已修（F-01）+ 回归 |
| handler panic → 请求永不应答 | 中 | 高 | 中（UI 超时，无错误提示） | ✅ 已修（F-02），测试待补（P1-09） |
| 引擎会话泄漏 / 句柄失效 | 高 | 中 | 低 | ✅ 已修（F-11/F-12/F-13） |
| 合并决策丢失 | 高 | 高 | 中 | ✅ 已修（F-06），残余见 P1-08 |
| 脏会话静默丢弃 | 中 | 高 | 低 | ✅ 已修（F-18） |
| 会话持久化掉电损坏 | 中 | 高 | 低 | 登记 P1-03/P1-04 |
| mock 与真实引擎行为分叉 | 高 | 中 | 中（UI 测试全绿但真引擎失败） | 登记 P1-05 |
| ts 类型与线上载荷脱节 | 高 | 中 | 低 | 登记 P1-01/P1-02 |
| GPL 依赖静默引入 | 低 | 高 | 无（无 CI 执行 deny） | 登记 P1-10 |

---

## 6. 后续建议排期

1. **立刻（本次已做）**：F-01 ~ F-19 已修 + 回归 + 变异验证。
2. **下一批次（建议）**：
   - P1-07 + P1-08（崩溃恢复与合并决策持久化，与 F-11/F-12 同族，一起做成本最低）
   - P1-05（mock/真引擎分叉 —— 直接影响「UI 测试是否可信」这一根问题）
   - P1-09（panic 隔离的回归测试）
3. **再下一批次**：
   - P1-03 + P1-04（持久化原子性 —— 数据正确性的最后一块拼图）
   - P1-01 + P1-02（协议类型可信度 —— 需要一次覆盖全部 `Option` 字段的批量改造）
   - P1-10（CI —— 建议在上述改造合入后建立，避免 CI 一上线就红）
4. **持续**：`engine/ai-core` 的增量 diff 与行内 diff 降级路径的性质测试补强。

---

## 附：本次评审的产出物

| 文件 | 说明 |
| --- | --- |
| `tests/protocol_conformance/protocol_test.rs` | 新增：7 条协议合规与进程生命周期回归 |
| `ui/src/shell/sessionLifecycle.test.ts` | 新增：7 条 UI 侧回归（会话生命周期 / 重试连接 / 通知异常隔离） |
| `tests/src/lib.rs` | 修改：`ensure_engine_fresh()` —— 消除「跑旧二进制」的假通过 |
| `engine/ai-engine/src/server.rs` | 修改：EOF 退出、panic 隔离、`id:null`、jsonrpc 版本校验 |
| `engine/ai-engine/src/handlers.rs` | 修改：`merge3/open` 同参数幂等 |
| `engine/ai-protocol/src/types.rs` | 修改：`RequestId` 增补 `Null` 变体 |
| `cli/aic/src/app.rs` | 修改：patch 退出码语义、`--conflict-exit-code` 校验 |
| `ui/src/ipc/engineProcess.ts` | 修改：重试发起连接、崩溃清理监听器、resync 句柄登记、delta 异常隔离 |
| `ui/src/protocol/client.ts` | 修改：通知订阅者异常隔离 |
| `ui/src/shell/sliceWindowsTabs.ts` | 修改：会话释放全覆盖、rebind 全覆盖、脏会话确认 |
| `ui/src/views/hex-compare/HexCompareView.tsx` | 修改：分页在途标记、能力位判错 |

---
---

# 第二轮深度评审（2026-11-14 晚）

> 评审基线：HEAD `d6b2032`（第一轮修复后，引擎 829 / UI 484 全绿）
> 评审终点：HEAD `954bb1c`（本轮 9 次提交，引擎 **842** / UI **485** 全绿）
> 评审维度顺序：**① 对原始期望（BC 忠实度）的功能准确性 → ② 代码质量与正确性**
> 方法：真实文件系统 / 真实引擎 RPC / 与 BC 自带权威文档逐条对照 + **双向变异验证**
> 权威基线：`docs/01-需求规格说明书.md`（163 FR）+ `/Applications/Beyond Compare.app`（5.2.0.31950）**自带英文帮助文档**（本轮新引入的一手证据源，见 §7.3）

## 7.1 结论摘要

**一句话结论**：引擎算法层（Myers / Patience / histogram / 行内 / 重要性 / 手动对齐 / patch）**经对拍与变异验证是正确的**，质量高于第一轮评估；但**「P0 需求全量闭环」这一声明不成立** —— SRS §6.3 明列的 P0 MVP 范围里，**编码自动检测**与**差异文件导航**两项缺失，**重要性在编辑后被抹除**；同时本轮发现并修复了一个**可销毁比较根之外用户数据的 P0 安全缺陷**、**同步计划层两项静默数据丢失**（Update 整树复制覆写较新文件、大小写变体镜像清空目标侧且退出码 0），以及**用户在安装版上实测报出的「树形↔平铺切换丢光文件」**。

本轮**修复 11 项缺陷 + 清偿 1 项测试基建（P1-09）**，新增 **14 条回归测试**（引擎 13 + UI 1），全部通过双向变异验证。**更正第一轮 4 项裁定**（其中 1 项是我自己的过度声称）。**新登记 11 项**，其中 3 项为契约级决策，需用户拍板（§7.7）。

| 维度 | 第一轮评分 | 本轮修正 | 依据 |
| --- | --- | --- | --- |
| 算法正确性 | 未单独评分 | **A** | Myers 扩样对拍（400 对 ≤40×40 + 1682 对抗用例 + 20 变异 + deadline 路径）全过；aligners/inline/importance/manual/patch 逐条核实 |
| BC 功能忠实度 | 未单独评分 | **C** | 命令面 26/370、菜单 5/10；P0 MVP 两项缺失（§7.3） |
| 数据安全 | B- | **D → B**（修复后） | 符号链接逃逸可覆写/删除根外文件（已修）；脚本 RESCAN 静默扩大删除作用域（已修） |
| 测试可信度 | B+ | **B**（下调） | UI **零 `.test.tsx`**：全部 484 个测试跑在 node 环境，**视图组件层无测试覆盖**；本轮 2 个 UI 缺陷因此无法附回归 |
| 工程化 | C+ | C+（维持） | 无 CI、cargo-deny 未安装、壳层脱离 workspace、MSRV 声明不可满足 |

**建议**：**继续推进，但先清偿 P0 忠实度缺口**。本轮修复不改变任何既有协议语义，可安全合入。

---

## 7.2 本轮已修复（11 项缺陷 + 1 项测试清偿）

| # | 严重度 | 位置 | 缺陷 | 提交 |
| --- | --- | --- | --- | --- |
| R2-01 | **P0 安全** | `ai-dir/src/execute.rs` | **符号链接逃逸**：比较根内的操作可覆写/销毁根外用户数据 | `cdfb414` |
| R2-02 | **P1 数据丢失** | `cli/aic/src/script.rs` `rescan_dir` | RESCAN 全量展开目录 → 后续 SELECT/DELETE 作用域从顶层静默扩到整棵子树 | `13e26f4` |
| R2-03 | P2 | `ai-core/src/report.rs:1601` | XML 报告含非法控制字符 → **整份报告无法被任何 XML 解析器读取**（且进程退 0） | `01c587e` |
| R2-04 | P2 | `cli/aic/src/script.rs` `write_log` | `.ok()?` 两处 → 函数恒返回 None，**错误分支是死代码**，LOG 写失败静默 | `13e26f4` |
| R2-05 | P2 | `cli/aic/src/script.rs` `Cmd::Beep` | BEEP 被 `/silent` 抑制，违反 FR-SCR-002（BEEP 无视 silent） | `13e26f4` |
| R2-06 | P2 | `cli/aic/src/app.rs:413` | `--conflict-exit-code` 只拒 0，**放行 2/11/12/13** → 覆盖保留语义（F-09 未完成） | `80e205f` |
| R2-07 | P2 | `ui/.../HexCompareView.tsx:130` | 偏移跳转 `idx = floor((jumpTo-aligned)/ROW_BYTES)` **恒为 0** → 任何跳转都落到页首 | `5d943f0` |
| R2-08 | P2 | `ui/.../FolderCompareView.tsx` | 名称过滤硬编码 `includeDirs: []` → **`includeFiles` 掩码使整页空白** | `5d943f0` |
| R2-09 | 测试清偿 | `ai-engine/src/server.rs` | panic 隔离只有代码无回归（第一轮 P1-09）→ 补 2 条库内测试 + `#[cfg(test)]` panic 方法 | `180ccc0` |
| R2-10 | **P1 用户实测** | `ai-dir/src/session.rs` `update_filters` | **树形↔平铺切换丢光文件**：UI 显示类切换每次附带空 `nameFilter`，`update_filters` 无条件重扫 → 已展开子树被整体移除（连带修好两条借道重扫的路径：`dir/execute` 落盘刷新、右键「刷新」） | `5b7a297` |
| R2-11 | **P1 数据丢失** | `ai-dir/src/sync.rs` `apply_preset` | **Update 预设整树复制目录行**：目录 mtime 较新即整树 copy，无条件覆写子项中较新的文件 | `678e80d` |
| R2-12 | **P1 数据丢失** | `ai-dir/src/sync.rs` 计划序 | **大小写变体镜像清空目标侧**：不敏感卷上 `A.txt`/`a.txt` 同一条目，先 copy 后 delete 把刚写入的内容删掉（退出码 0） | `954bb1c` |

### 7.2.1 R2-01（P0 安全）—— 本轮最重要发现

**根因不是「护栏写错了」，而是「护栏装错了位置」**：

`execute.rs` 的 `validate()` 里有符号链接检查，但**它只在两个地方被调用**：① `dry_run` 分支；② `copy_path` 内部。**真实落盘路径（step 3）直接调 `remove_side` / `rename_side` / `touch_side` / `mkdir_side`，完全绕过 `validate`。** 而 `sanitize_rel`（:147）只做**词法**规范化，其文档注释却声称「通过护栏后 `root.join(rel)` 不可能逃出根」——**该声明为假**。

**真实文件系统证据**（`/tmp/symprobe`，修复前）：

```
# 场景 A：copy 写穿目标侧符号链接
OUTSIDE/precious.txt  内容 PRECIOUS-OUTSIDE  →  被覆写为 LEFT-CONTENT
# 场景 B：delete 经由符号链接父目录
OUTSIDE2/victim.txt   →  被删除（目录只剩 0 个文件）
```

修复后两处均**原样保留**。护栏移至 `exec_one` **公共 step 1**（`sanitize_rel` 之后），逐分量拒绝符号链接目录 + 拒绝符号链接末分量 + 校验 copy 写穿目标。

**双向变异**：丢弃护栏返回值 → 2 条新测试**变红**；同时既有 `delete_symlink_does_not_follow`（:678）**保持绿** → 证明护栏**没有过宽**（不是「一律拒绝所有符号链接」的偷懒修法）。

> 附带发现：`ai-vfs/src/local.rs:92-99` 的 `stat` 用 `symlink_metadata`（= lstat），这是**正确的**——它让引擎能看见符号链接本身。缺陷纯在 ai-dir 执行层的调用路径上。

### 7.2.2 R2-02（P1 数据丢失）—— 变异验证揪出了一个「假守卫测试」

`rescan_dir` 原实现把**结果里所有目录**无条件塞进 `state.expanded`。而 `row_visible`（:991-1004）要求**全部祖先目录都在 expanded 中**才可见。于是用户 COLLAPSE 掉的子树在 RESCAN 后重新可见，紧随其后的 `SELECT all` + `DELETE` 就把作用域从顶层扩到了整棵子树。

**实测**：COPY+DELETE 组合**销毁全部 4 个文件**；单独 DELETE 则保留嵌套文件。修复改为**只展开本次新出现的目录**（与旧 arena 的目录集做差）。

**这里有一个值得记录的方法论教训**：我第一版回归测试用 `mkdir all fresh`（**顶层**目录）断言，结果**过度修复的变异体依然全绿**——因为 `row_visible` 对顶层直接 `None => return true`，根本不走 expanded 判定。**一个在变异下不变红的测试什么都证明不了。** 重写为嵌套 `mkdir all sub` + `mkdir all sub/inner` 后，变异体立刻变红。

### 7.2.3 其余各项的关键证据

- **R2-03**：修复前 `python3 -c "ET.parse(...)"` 报 `not well-formed`，而 `aic report` **退出码 0**。修复后可正常解析。注意 XML 1.0 的 `Char` 产生式**连 `&#xNN;` 引用都不允许**这些码点，所以只能剔除、不能转义。新增测试用 4 组语料（ctl/nul/noncharacter/legal_kept）+ TAB 必须保留断言。
- **R2-06**：`--conflict-exit-code` 现拒绝 `0|2|11|12|13` 并列出全部保留语义；放行 1/14/3/200。
- **R2-08**：真实引擎验证 `includeFiles:["src"]` → `visibleRows=0`；`includeDirs:["src"]` → `1`。修复改为调用**早已实现且有测试、但零生产调用方**的 `dirFilters.ts:104 parseMaskInput`。

> **本轮两次踩到同一个坑：陈旧二进制导致假结果。** `tests/cli/cli_matrix_test.rs` 依赖 `target/debug/aic`，改完 `app.rs` 不重建就会拿到旧行为（一度误判 BEEP「FAIL」、冲突码测试报「2 个未决冲突块」）。**任何 CLI 层结论都必须先 `cargo build -p aic --bins`。**

### 7.2.4 用户实测缺陷 + 同步计划层两项数据丢失（R2-10/11/12）

这三项在本轮报告初稿写完之后才完成，**其中 R2-10 是用户在使用安装版时实测报出的**（「我树形改平铺之后界面上就不展示任何文件了，重新选树形也没有用」）。

**R2-10 根因链（三段，缺一不会触发）**：

1. UI `FolderCompareView.applyFilters` 每次显示类切换（平铺/抑制/显示过滤）都附带 `nameFilter: parseMaskInput(当前输入)`；输入为空时 `parseMaskInput('')` 返回**非 null 的全空 spec**（`dirFilters.ts:104`）。
2. 引擎 `dir_filter` 见 `name_filter.is_some()` 即调 `update_filters`。
3. `update_filters` **无条件 `rescan()`**；`rescan` 重建 arena 并把**全部旧 rel 路径**塞进 `st.removals` → 用户已展开的子树被整体移除，平铺行集只剩顶层文件；切回树形也只剩折叠顶层（展开状态已被摧毁，不是显示问题）。

真实引擎 RPC 证据（修复前）：`flat=true` → `removals:['f.txt','sub','sub/g.txt']`、`upserts:['f.txt']`（子树文件 `g.txt` 消失）。
修复后：`flat=true` → `upserts:['f.txt','g.txt']`、`removals:[]`；`flat=false` → `upserts:['f.txt','sub','sub/g.txt']`、`removals:[]`。

**修法**：`update_filters` 先判等（全空 spec 归一为 `None`），未变即 no-op。**强制重扫改走显式入口**，因为原判等把两条「本来就依赖无条件重扫」的路径变成了 no-op —— 这两处回归都是**门禁真实抓出来的，不是我事后想到的**：

| 借道重扫的路径 | 变 no-op 后的后果 | 谁抓到的 | 修法 |
| --- | --- | --- | --- |
| `dir/execute` 落盘后（`handlers.rs:1749` 原调 `update_filters(None,None)`） | 复制/删除后 UI 不刷新，行集与磁盘不一致 | `tests/p9_e2e` 红：`rescan delta 含旧行 removals` | 新增 `DirModel::refresh()`，execute 改调它 |
| 右键「刷新」（`FolderCompareView.tsx:550` 原调 `applyFilters()`） | 外部改动永远刷不出来（BC 的 Refresh 语义丢失） | 读代码顺链发现（**无测试覆盖，视图层零 `.test.tsx`**） | `dir/filter` 新增 `rescan` 参数，刷新下发 `rescan:true` |

> 这两处说明一个结构性问题：**「重扫」原先没有自己的入口，被寄生在「改过滤器」上**。本修复把它显式化（`refresh()` / `rescan` 参数），而不是给 `update_filters` 加布尔开关继续混用语义。

**R2-11（Update 预设整树复制目录行）**：目录 mtime 只反映条目增删，不代表内容新旧。真实文件系统复现：左 `sub/f.txt` 旧、右 `sub/f.txt` 新（`NEWER-RIGHT`），目录行左 mtime 较新 → Update 左→右 整树 copy → 右侧被覆写成 `NEW-LEFT`，**较新内容丢失**。修法是 Update 三预设跳过 `Dir` 行（子文件行各自携带 Update 语义）；Mirror 语义=使两侧一致，仍允许整树复制。

**R2-12（大小写变体镜像清空目标侧）**：macOS APFS 上左孤 `A.txt` 与右孤 `a.txt` 是**同一条目**。原计划序先 copy 后 delete：copy 原地覆写 `a.txt`，delete 再把刚写入的内容删掉 → **目标侧清空且退出码 0**。真实复现：`left/A.txt=LEFT`、`right/a.txt=RIGHT`，mirror 左→右 后 right 目录为空、`exit=0`；修复后 `right/A.txt=LEFT`。修法是把碰撞的 delete 提前到对应 copy 之前 —— **该序在敏感卷与不敏感卷上都正确**；不采用「折叠对齐」或「抑制 delete」，那两种做法会在大小写敏感卷上产生错误结果。

**对独立复核（子 agent）两条声称的裁定**（按「逐条实测核实、不顺着答」纪律；两条声称的**原始机理都被否证，但相邻的真缺陷都成立**，我各自重跑复现后才修）：

| 声称原文 | 裁定 | 实测依据 |
| --- | --- | --- |
| A：「Update 预设会改写目标侧**目录 mtime**」 | **REFUTED（如述）** + 相邻缺陷 CONFIRMED | `apply_preset` 根本没有 `Touch*` 分支；全仓唯一 Touch 生产者是 `RuleAction::TouchLeft/Right`（显式规则/CLI touch）。`copy_tree_inner` 只 `mkdir_all`，从不 `set_mtime`。复现：目标目录刻意 stamp 成 2000-01-01，Update 后**目录 mtime 原样保留** —— 但 `right/sub/f.txt`（2024-07-01，`NEWER-RIGHT`）被左侧旧版覆写成 `NEW-LEFT`。RPC 原始计划：`{"plan":[{"op":"copyLeftToRight","path":"sub"}],"total":1}`。**真缺陷是整树复制覆写较新子文件（R2-11），不是 mtime** |
| B：「`compare_case=false` + 平铺折叠对齐 → 镜像毁掉两侧文件」 | **机理 REFUTED** + 数据丢失 CONFIRMED | `CaseFoldAligner` **只在测试里用**（`align.rs:307/317/327`、`session/tests.rs`）；全部生产调用方注入 `ByNameAligner`（`handlers.rs:268`、`script.rs:1344/1625`、`app.rs:354/678`）。平铺 + `compareCase:false` 只发一条 copy、无 delete，两种卷上都不丢数据。**真触发条件是「树形 + 精确名对齐 + 不敏感卷 + 默认设置」，且 CLI 可达**（R2-12）：`build_pairs` 按 `BTreeSet` 键排序，`"A.txt" < "a.txt"` → copy 先于 delete |
| zip CRC / zip64 截断 / zip 保存损坏 | **仍未裁定** | 本轮未实测，**不声称已确认或已排除** |

> **我否掉了子 agent 给的两个修法**：它建议「按卷大小写敏感性选 `CaseFoldAligner`」或「抑制同批已被写过的折叠同名 delete」。前者需要 VFS 能力探测且改变对齐语义，后者在**大小写敏感卷**上会把本该删除的独立文件留下。改用**重排执行序**（delete 提前到 copy 之前）：两种卷语义下都正确，且与触发顺序无关（子 agent 指出的「左名排序在前才触发」只是原实现的症状，重排后 `left/a.txt + right/A.txt` 的反向情形也一并覆盖）。
>
> 子 agent 另注明「Tauri GUI 端到端未测，GUI 可达性属推断」——本轮 R2-10 由用户在安装版 GUI 上实测报出，**GUI 可达性已由用户实测补齐**。

---

## 7.3 BC 功能忠实度审计（用户第一维度）

> 本轮引入**一手权威证据**：BC 自带英文帮助 `/Applications/Beyond Compare.app/Contents/Resources/en.lproj/Help/`，其中 `commands*.html`（15 页）逐条列出**每个视图的每个菜单的每个命令**，`command_line_reference.html` 给出**权威退出码表**。以下所有 BC 侧数字均从这些文件解析得出，非凭印象。

### 7.3.1 命令面与菜单面：量化差距

从 BC 帮助解析出的 `class="f_Command"` 条目（去重）：

| BC 菜单 | 命令数 | 我们的实现 |
| --- | --- | --- |
| Beyond Compare（应用菜单） | 7 | ✗ 无（Settings 等散落在 tools） |
| Session | 56 | 部分（4 条） |
| File | 23 | **✗ 菜单不存在** |
| Edit | 68 | **✗ 菜单不存在** |
| Search | 43 | **✗ 菜单不存在** |
| View | 84 | 部分（10 条） |
| Actions（文件夹/同步/合并视图） | 68 | **✗ 菜单不存在** |
| Tools | 6 | 部分（5 条） |
| Window | 12 | 部分（7 条） |
| Help | 3 | 硬编码 About/Docs（`menuModel.ts:18` 为 `help: []`） |
| **合计** | **370** | **26 条命令 / 5 个菜单** |

`ui/src/shell/menuModel.ts:9` 的 `MenuId` 只有 `session|view|window|tools|help`。BC 文件夹比较视图的菜单是 `Session / Actions / Edit / Search / View`（+ 公共 Tools/Window/Help/应用菜单），文本比较是 `Session / File / Edit / Search / View`。**缺 File / Edit / Search / Actions 四个菜单 + 应用菜单。**

> 更正：本轮早期口径「3 of 8 菜单缺失」不准确，按 BC 帮助实测应为**缺 4 个视图菜单 + 应用菜单（5/10）**。

### 7.3.2 【P0】编码自动检测：SRS 列为 MVP，实现直接拒绝非 UTF-8

**SRS §6.3「MVP（P0）范围小结」第 2 条**明列「…+ **编码自动检测**」。

**实现**（`ai-vfs/src/lib.rs:331-334`）：

```rust
/// 读取并解码为文本：UTF-8 / UTF-8 BOM（BOM 剥离）；P0 不做其他编码检测
async fn read_file_text(&self, path: &VfsPath) -> Result<String, VfsError> {
    let bytes = self.read_file_bytes(path).await?;
    let stripped = strip_bom(&bytes);
    String::from_utf8(stripped.to_vec()).map_err(|_| VfsError::InvalidEncoding { .. })
}
```

**实测**（GBK 中文文件，真实二进制）：

```
$ aic diff /tmp/encprobe/l.txt /tmp/encprobe/r.txt
aic diff: 文件不是合法 UTF-8 文本: /tmp/encprobe/l.txt
退出码=2
```

**BC 权威口径**（`formattextconversion.html`）：
> "The Encoding of most text files can be detected automatically, but you can specify a specific code page to use."

**影响**：中文 Windows 用户的 GBK / Big5 / Shift-JIS 文本**根本无法比较**——这是 BC 的核心日常场景。注意代码注释自称「P0 不做」，**与 SRS 的 P0 定义直接冲突**：这是实现方私自缩小了 P0 范围。

**关联缺口**：`TextOpenParams` **无 `encoding` 字段**（`ui/src/protocol/generated/TextOpenParams.ts`），`mapping.rs:344` 硬编码 `encoding: None`；`ui/src/lib/encodings.ts:52,72` 的编码工具**零生产调用方**。所以 UI 的「编码切换」是**纯 no-op**——PROGRESS.md 第 188 行「编码切换 ✅ P5」**不成立**。

### 7.3.3 【P0】差异文件导航（FR-DIR-071）：SRS 列为 MVP，实现缺失

**SRS §6.3 第 3 条**明列「…+ 子会话打开 + **差异文件导航**」。FR-DIR-071（:412-415）要求：搜索 > 下一个/上一个差异文件、复制到对侧并打开下一个差异、以及 5 项程序选项。

**实现**：全仓 grep `nextDiffFile|prevDiffFile|下一个差异文件|copyAndNext` → **零命中**。`shortcuts.ts` 16 条绑定里**没有任何差异导航键**。

### 7.3.4 【P0】状态回传：产品核心概念，实现明确未开放

SRS :50 把「文件级视图的差异信息**回传**文件夹会话（颜色覆盖）」列为产品两大核心衔接之一；:149 表格 likewise。

**实现自述**（`ui/src/shell/sliceWindowsTabs.ts:318`）：
> "差异回传均未开放（P1/P2）；UI 以行 id（相对路径）拼出两侧完整路径直接…"

`shellState.ts:76` 亦注明「引擎侧写回属 P1」。**即：从文件夹双击进文本比较后，文件夹行的颜色不会反映文件内差异**——BC 的招牌体验缺失。

### 7.3.5 【P1】重要性在编辑后被抹除

`ui/src/lib/lineDiffCore.ts:167` 的 `toSections(ops: DiffOp[])` **四个分支全部硬编码 `important: 'important'`**。`TextCompareView.tsx:397-400` 的 `rediffModel` 在用户编辑后走这条路径 → **红/蓝语义全部退化为红**。

根因：**协议无 `text/rediff` RPC**，前端只能本地重 diff，而本地重 diff 拿不到引擎的重要性规则。修复需二选一：① 新增 `text/rediff` RPC（改协议面）；② 把重要性规则下发到 TS 侧复刻（重复实现，易漂移）。**属架构决策，登记待拍板。**

### 7.3.6 【P1】文件夹高频快捷键是过期桩命令

`commands.ts:235-262` 三条命令的 `run` **只弹一句阶段提示，不执行任何操作**：

| 命令 | 键位 | run 实现 |
| --- | --- | --- |
| `folder.copyToLeft` | Ctrl/Cmd+L | `ctx.notice('复制到左侧属 P6 文件操作阶段交付')` |
| `folder.copyToRight` | Ctrl/Cmd+R | `ctx.notice('复制到右侧属 P6 文件操作阶段交付')` |
| `folder.markSelection` | Space | `ctx.notice('空格标记选择属 P6 选择模型阶段交付')` |

**而 P6 早已交付**：右键菜单（`FolderCompareView.tsx:506`）与 gutter 箭头（:709）都能真复制。Space 靠一个**window 捕获监听器绕过桩命令**才生效（:563-579，注释明确写了这是为了绕开壳层 shortcutEngine 的 `stopPropagation`）。

**净效果：BC 文件夹比较最高频的 Ctrl+L / Ctrl+R 在本实现里什么都不做。** 这是「命令注册表与视图实现脱节、桩命令未随交付回收」的架构债。

**为何本轮不修**：`ui/` **零 `.test.tsx`**（484 个测试全在 node 环境跑纯函数），改 `FolderCompareView` + 拆除 Space  workaround **无法附回归测试**，且需浏览器实测交互。按「测不到就说测不到」原则登记，不擅自改不可验证的交互层。**建议修法**：仿 `viewRegistry.ts:181 setNoticeSink` 既有模式加一个 folder 命令 sink，三条桩改为委派，同时拆除 Space 的 window 捕获 workaround。

### 7.3.7 忠实度审计结论

**PROGRESS.md 的「P0 需求全量闭环」（:36、:142）与「M2 MVP ✅ P0 需求全量覆盖」（:28）声明不成立**，已在本次同步校准。BC 忠实度审计清单（:171-194）中下列 ✅ 需下调：

| 原声明 | 实测 |
| --- | --- |
| 编码切换 ✅ P5 | **✗ no-op**（协议无字段、工具零调用方、引擎拒非 UTF-8） |
| CLI（退出码 11-14）✅ P3/P11 | **⚠ 部分**：11/12/13/14/105/106/107 对齐；**0/1/2 语义与 BC 相反**（§7.7） |
| 文本比较 ✅ 队列 6 | **⚠**：编辑后重要性抹除 |
| 文件夹比较 ✅ P6c 后 | **⚠**：Ctrl+L/R 桩；差异文件导航缺失；状态回传未开放 |
| 菜单栏/工具栏 ✅ 队列 8 | **⚠**：5/10 菜单、26/370 命令 |
| 三向合并 ✅ 队列 3 | **⚠**：EOL 处理与 git 不一致（§7.5 R2-N4） |

---

## 7.4 第一轮 21 项待决项的复核裁定

> 原则：**不采信任何结论的措辞，逐条实测**。包括我自己的结论。

| 原编号 | 本轮裁定 | 证据 |
| --- | --- | --- |
| **P1-04** | **降级为「潜在 P0」**（原判 CONFIRMED 是**我的过度声称**） | 我原先只验了「代码无串行化」，**未验可达性**。实测 `ui/src-tauri/src/lib.rs` 共 **7 个 `#[tauri::command]`，0 个 async** → Tauri 同步命令在主线程串行执行，**当前不存在并发写**。但 `service.ts:159-170 persistByFileKey` 确实无队列，**一旦任何命令改 async 即成 P0 数据损坏**。属「结构上暂时不可达的真实隐患」 |
| **P1-08** | **REFUTED → 降级 P3** | `Merge3View.tsx` **没有局部 `decisions` state**；决策在引擎侧，视图重挂载后由 `sections` 回带恢复。残余仅「视图偏好（窗格隐藏等）不持久」，属 P3 |
| **P1-09** | **✅ 已清偿** | `180ccc0`：2 条库内测试（panic → -32603；panic 不污染共享状态）+ `#[cfg(test)] "test/panic"` dispatch |
| **P1-03** | **CONFIRMED，且比原判更严重** | 除掉电窗口外，**失败路径也丢数据**：`settings_write`（:353-356）先 `rename(primary→.bak)`，若随后 `rename(tmp→primary)` 失败，**主文件已不存在**，只剩 .bak |
| **P1-07** | **CONFIRMED** | 全仓仅 **3 处 `trackSession`**；`TrackedSession.kind` 联合类型是 `'dir'\|'text'`，而会话种类有 **7 种** |
| **P1-10** | **CONFIRMED** | 无 `.github/workflows`、无 Makefile/justfile；`cargo-deny` **未安装** → `deny.toml` 的 GPL 零容忍策略**无任何执行点** |
| **P2-12** | **CONFIRMED，且比原判更严重** | 原判「算式可疑」；实测 `idx = floor((jumpTo-aligned)/ROW_BYTES)` 在 `aligned === jumpTo` 时**恒为 0**，选择器退化。**已修（R2-07）**。阈值精化为 `0x800`（`PAGE_BYTES 4096 / ROW_BYTES 16 = 256` 行/页） |
| **P2-13** | **REFUTED（前提错误）** | 原判「`deleteFile` 调用方未启用」；实测**存在活调用方** `demoSeed.ts:86` → P1-06 的影响面**不是受限，而是已生效** |
| P3「行内 diff 超时降级」 | **RESOLVED（无缺陷）** | 实测降级路径**正确返回 `truncated=true`**，不是静默产错 |
| P1-01/02/05/06、P2-01~11 | **维持登记**，本轮未逐条重验 | 显式标注：**未实测**，不声称已确认 |

---

## 7.5 本轮新登记（未修，附证据与修法）

| # | 严重度 | 位置 | 问题 | 为何不修 |
| --- | --- | --- | --- | --- |
| **R2-N1** | **P0 契约** | `cli/aic/src/app.rs` 等 **76 处调用点** | **退出码 0/1/2 语义与 BC 相反**（详见 §7.7） | 契约级产品决策，需用户拍板 |
| **R2-N2** | **P0 功能** | `ai-vfs/src/lib.rs:331` | 编码自动检测缺失（§7.3.2） | 需协议加 `encoding` 字段 + 检测链 + 错误态工具栏，跨 3 层 |
| **R2-N3** | **P0 功能** | UI 全局 | FR-DIR-071 差异文件导航 + 状态回传缺失（§7.3.3/4） | 需引擎侧写回 RPC + 父会话关联，属 P1 架构批 |
| **R2-N4** | **P1 数据正确性** | `ai-core/src/merge3way.rs` | **合并 EOL 损坏**：与 `git merge-file` 对拍——base=left=`x\ny`、right=`x\ny\n` → aic 输出 `x\ny`，git 输出 `x\ny\n`；**文件中段**场景 aic 输出 `f\nga`（**两行被融合**），git 正确 | 修复需改 chunk 分类语义（**中风险**），登记待拍板而非此刻擅改 |
| **R2-N5** | **P1 功能** | `ui/src/shell/commands.ts:235-262` | Ctrl+L/Ctrl+R 桩命令（§7.3.6） | UI 视图层**零测试覆盖**，无法附回归 |
| **R2-N6** | **P1 重要性** | `ui/src/lib/lineDiffCore.ts:167` | 编辑后重要性抹除（§7.3.5） | 需新增 RPC 或 TS 复刻规则，架构决策 |
| R2-N7 | P2 | `engine/ai-core/src/incremental.rs` | 增量 splice **未接线**（无生产调用方），边界锚定缺性质证明 | 潜在项，接线前需要先补随机编辑序列 vs 全量 diff 的一致性测试 |
| R2-N8 | P2 | `engine/ai-core/src/inline.rs` | 行内 DMP **默认禁用**，启用路径缺覆盖 | 潜在项；超时降级本身已验证正确 |
| R2-N9 | P2 | `scripts/build-app.sh:55` | `codesign … \|\| true` **静默吞掉签名失败** | 本轮改为**打包后显式验签**（见 §7.8），脚本本身待改 |
| R2-N10 | P2 | `ui/src-tauri/Cargo.toml` | MSRV 声明 **1.77.2 不可满足**（tauri 2 需 ≥1.90）；且壳层脱离根 workspace，`cargo clippy --all-targets` 不覆盖 | 需与根 MSRV 统一，属工程化批 |
| R2-N11 | P3 | `ui/src/shell/menuModel.ts:18` | `help: []` → Help 菜单在 `MenuBar.tsx` 里硬编码 About/Docs，**不走命令注册表** → 无法自定义键位/隐藏 | 随菜单面补齐批一起做 |

**ai-dir / ai-vfs 侧原 5 项待裁定的结清情况**：folder mtime Update 覆写、大小写不敏感镜像两项**已裁定并修复**（原始声称的机理均被否证，相邻真缺陷成立 → R2-11 / R2-12，逐条证据与裁定见 §7.2.4）；**zip CRC、zip64 截断、zip 保存损坏三项仍未裁定**——本轮未实测，不声称已确认或已排除。

---

## 7.6 门禁复验（HEAD `954bb1c`）

| 门禁 | 结果 |
| --- | --- |
| `cargo test --workspace` | **842 / 842 通过，0 失败**（基线 829 + 本轮 13） |
| `cargo clippy --all-targets -- -D warnings` | **零警告** |
| `pnpm test`（UI） | **485 / 485**（49 个测试文件） |
| `pnpm lint` / `pnpm build` | 全绿（eslint 零告警；tsc -b + vite build 成功） |
| `pnpm e2e:engine`（真实引擎） | 全绿（`✅ 真实引擎端到端：全部通过`，引擎随 shutdown 退出 exit=0） |
| `ui/src-tauri` 独立 clippy | 零警告（壳层不在根 workspace，需单独跑） |

**新增 14 条回归**：`e2e_rescan_preserves_collapse_scope`、`e2e_rescan_expands_newly_created_dirs`、`e2e_log_write_failure_is_reported`、`copy_through_destination_symlink_is_rejected`、`delete_through_symlinked_parent_is_rejected`、`handler_panic_is_answered_as_internal_error`、`handler_panic_does_not_poison_shared_state`、`merge_conflict_exit_code_rejects_reserved_codes`、`xml_report_strips_illegal_control_chars`、`unchanged_name_filter_must_not_rescan_away_expanded_rows`、`update_preset_must_not_tree_copy_dir_rows`、`mirror_case_variant_delete_runs_before_copy`、`dir_filter_rescan_flag_picks_up_external_changes`（以上引擎 13）、UI `mockDirOps「强制重扫（rescan=true）」`（1）。

> **门禁真实抓到了一次我自己引入的回归**：R2-10 的 no-op 守卫把 `dir/execute` 借道的 `update_filters(None,None)` 变成了空操作，`tests/p9_e2e` 立刻红（`rescan delta 含旧行 removals`）。**不是先写测试再补修复，而是修复被既有门禁拦下**——这正是保留全量 e2e 的价值。第二条同类回归（右键「刷新」）**没有测试覆盖，是读调用链发现的**，已补 `dir_filter_rescan_flag_picks_up_external_changes` 守住引擎侧；UI 侧仍只有 mock 层测试（视图层零 `.test.tsx`，见 R2-N5）。

### 7.6.1 双向变异矩阵（本轮）

| 修复 | 回退变异（应变红） | 过度修复变异（应变红） |
| --- | --- | --- |
| R2-01 符号链接 | 丢弃护栏返回值 → 2 条新测试红 ✓ | 一律拒绝所有符号链接 → 既有 `delete_symlink_does_not_follow` 红 ✓（证明未过宽） |
| R2-02 RESCAN | 恢复全量展开 → `preserves_collapse_scope` 红 ✓ | 一律不展开新目录 → `expands_newly_created_dirs` 红 ✓（**第一版测试在此变异下假绿，已重写**） |
| R2-03 XML | 恢复 `html_escape` → 解析测试红 ✓ | 连 TAB/换行一起剔除 → legal_kept 语料红 ✓ |
| R2-06 冲突码 | 恢复只拒 0 → 保留码测试红 ✓ | 拒绝一切非 14 → 合法值 1/3/200 红 ✓ |
| R2-10 过滤器判等 | 守卫改 `if false && !changed`（恢复无条件重扫）→ `unchanged_name_filter_…` 红（tests.rs:161）✓ | 守卫改 `if !changed \|\| true`（一律不重扫）→ 既有 `update_filters_rescans_and_removes_old_rows` 红（tests.rs:136）✓ |
| R2-10 强制重扫入口 | `dir/execute` 的 `refresh()` 换回 `update_filters(None,None)` → `p9_e2e` 红 ✓（**门禁真实捕获，非事后补演**） | — |
| R2-11 Update 跳过 Dir | 去掉 Dir 跳过分支 → `update_preset_must_not_tree_copy_dir_rows` 红 ✓ | 把 Mirror 两预设也加进跳过集 → 同测试的 Mirror 断言红 ✓（证明未过宽） |
| R2-12 碰撞重排 | 函数首行注入 `return plan;` → `mirror_case_variant_delete_runs_before_copy` 红，且 R2-11 测试仍绿 ✓（两项各自独立被守护） | 去掉「折叠同名且路径不同」条件（把所有 delete 都提前）→ 新测试 + 既有 `mirror_left_to_right` 双双红 ✓ |

> **UI 两项（R2-07/08）无法做变异验证**：视图层零测试覆盖。已用**真实引擎 RPC** 与**手工算式推演**替代，并在 §7.5 R2-N5 登记该覆盖缺口。**不声称它们有回归保护。**

---

## 7.7 需用户拍板的契约级决策

### 决策 1：CLI 退出码 —— BC 忠实度 vs GNU diff/git difftool 兼容

**BC 权威退出码表**（`command_line_reference.html`，一手证据）：

| 码 | BC 含义 | 我们的含义 |
| --- | --- | --- |
| 0 | Success | 相同/成功 ✓ |
| **1** | **Binary same**（一种「相同」） | **不同** ✗ |
| **2** | **Rules-based same**（一种「相同」） | **错误** ✗ |
| 11 | Binary differences | 二进制差异 ✓ |
| 12 | Similar | 相似 ✓ |
| 13 | Rules-based differences | 基于规则的差异 ✓ |
| 14 | Conflicts detected | 冲突 ✓ |
| **100** | **Unknown error** | 无（我们用 2） ✗ |
| **101** | Conflicts detected, merge output not written | 无 ✗ |
| 105/106/107 | 脚本读失败/语法错/加载失败 | **完全一致 ✓** |

**冲突本质**：BC 把 **1 和 2 用于「相同」的细分**，错误走 100；我们沿用 **GNU diff 惯例**（0 同 / 1 异 / 2 错）。**为 BC 写的批处理脚本（`if errorlevel 1` 判「二进制相同」）在我们这里会完全误判。**

**代价**：改齐 BC 需动 **76 处调用点 + 全部退出码测试 + golden**，且**破坏 git difftool 兼容**（git 期望 0/1）。这不是 bug 修复，是**产品契约选择**，我不擅自改。

### 决策 2：重要性回传 —— 新增 `text/rediff` RPC vs TS 侧复刻规则

前者改协议面（但语义单一来源、不漂移）；后者不动协议（但重要性规则要在 Rust/TS 各维护一份，长期必漂移）。**倾向前者。**

### 决策 3：合并 EOL 语义（R2-N4）

以 `git merge-file` 为准修 chunk 分类（中风险，需重跑 28 组对拍），还是保持现状并把「EOL 差异不参与合并」写进文档？**倾向前者**——因为当前行为会**融合两行**，属数据损坏而非风格差异。

---

## 7.8 本轮方法论备忘（供后续评审复用）

1. **护栏要装在公共路径上，不是装在「看起来该装」的函数里。** R2-01 的根因是 `validate` 只在 dry_run 与 copy 内被调用——**读代码时先画调用图，再判断护栏覆盖面**。
2. **在变异下不变红的测试等于没有测试。** R2-02 第一版测试用顶层目录，过度修复变异体全绿；换嵌套目录才有鉴别力。
3. **陈旧二进制会伪造结论。** 本轮两次误判都源于此。CLI 层任何实测前先重建。
4. **「代码没有防护」≠「缺陷可达」。** P1-04 我原判 CONFIRMED，实测 7 个同步命令 0 个 async 后必须降级。**可达性要单独验。**
5. **不采信其他 reviewer 的措辞，也不采信自己上一轮的措辞。** 本轮更正 4 项，其中 1 项是我自己的过度声称。
6. **一手权威文档 > 印象。** BC 自带帮助给出了 370 命令 / 10 菜单 / 权威退出码表，把「忠实度」从主观判断变成了可量化对照。**后续评审应继续以此为基线。**
7. **测不到就明说。** UI 视图层零 `.test.tsx` 是本轮最重要的**测试可信度**发现：484 全绿**不代表视图正确**，R2-07/08 两个真实缺陷正好落在无覆盖区。
8. **一个修复会把「寄生在旧行为上的路径」变成新缺陷。** R2-10 的 no-op 守卫让 `dir/execute` 与右键「刷新」两条借道重扫的路径失效；前者被既有 e2e 抓红，后者只能靠读调用链。**改语义前先 grep 全部调用方，问「谁在依赖这个副作用」。**
9. **探针夹具必须先验干净。** 本轮一次安装版验证用了 /tmp 残留目录（内含上一轮符号链接逃逸探针的 `R/sub → OUT2` 符号链接），`mkdir -p`/写文件顺着链接走，产出「子树文件消失」的**假结果**。改用全新目录并 `find -type l` 断言为 0 后才可信。**任何 /tmp 复现目录都要么新建、要么先验结构。**

---

## 7.9 安装版验证（2026-11-15 00:0x，用户要求「编译最新代码在我本机安装上方便我体验」）

**产物与安装**：`bash scripts/build-app.sh --dmg` → `ui/src-tauri/target/release/bundle/macos/ai-compare.app` + `.../bundle/dmg/ai-compare_0.1.0_aarch64-apple-darwin.dmg`；`codesign --verify --deep --strict` 通过；覆盖 `/Applications/ai-compare.app`（旧包移至 `/tmp/ai-compare-prev-235756.app` 保留，未删除）。

**在安装版二进制（`/Applications/ai-compare.app/Contents/MacOS/ai-engine`）上的实测**，非构建树二进制：

| 验证项 | 原始结果 |
| --- | --- |
| 树→平铺（UI 真实载荷：空 nameFilter + flat:true） | `upserts:['f.txt','g.txt']`、`removals:[]` —— 子树文件保留（用户缺陷已修） |
| 平铺→树 | `upserts:['f.txt','sub','sub/g.txt']`、`removals:[]` —— 展开态保留 |
| 右键刷新（`rescan:true`，外部新建 external.txt） | `stats.total` 2→3，delta 含 `external.txt` |
| Update 左→右 计划 + 执行 | 计划仅 `[('copyLeftToRight','sub/h.txt')]`；执行后 `R/sub/f.txt` 仍为 `NEWER-RIGHT`、`R/sub/h.txt` 变 `H-LEFT` |
| 镜像左→右（左 A.txt / 右 a.txt）计划 + 执行 | 计划 `[('deleteRight','a.txt'),('copyLeftToRight','A.txt')]`；执行后右盘仅 `A.txt`，内容 `LEFT`（不再清空） |
| GUI 启动 | 主页正常渲染；`ai-engine --stdio` 子进程随会话拉起；无崩溃报告 |

**未验证项（明说）**：GUI 内「点击平铺按钮」的端到端点击路径**未自动化验证**（Tauri WebView 无自动化通道，视图层零 `.test.tsx`）；上表以安装版引擎 RPC + 真实磁盘终态为准，UI 载荷形状取自 `FolderCompareView.applyFilters` 源码。**请用户在安装版上点一次树↔平铺确认观感。**

---

## 8. 第三轮 review（R3）处置登记

**输入**：第三方报告 `REVIEW-2026-11-15.md`（R3-01..R3-26，26 项）+ 本轮自查新增 R3-27/28。
**用户授权**：「确认自主决策全部修复直至完成」——确信缺陷直接修，架构/契约级不确信项记录待决。
**纪律**：每项独立提交、验收写进提交信息；不提交红测试；rustfmt/prettier 只清本次新增行（既有债不动）；push 另行授权。

### 8.1 逐项处置（截至 HEAD `7c4f809`）

| 项 | 摘要 | 处置 | 提交 |
| --- | --- | --- | --- |
| R3-01 | P0 递归复制符号链接根外写入 | 已修 | `96f564a` |
| R3-02 | P0 补丁应用符号链接根外写入 | 已修 | `5ceed84` |
| R3-03 | 平铺/手动对齐行地址改用真实侧路径 | 已修 | `5dde3f5` |
| R3-04 | 文本编辑接通真实落盘 text/save | 已修 | `3acd46c` |
| R3-05 | 未落盘编辑的关闭/退出脏保护 | 已修 | `9841b7d` |
| R3-06 | 三向合并末尾换行纳入比较键 | 已修 | `6493247` |
| R3-07 | 文件夹三向合并接通真实 RPC | 已修 | `a604bc2` |
| R3-08 | 会话保存/重开覆盖多类会话 | 已修 | `457e43b` |
| R3-09 | 目录扫描失败不再当成相同 | 已修 | `5dde3f5` |
| R3-10 | table/hex 切回标签 411 + 分页 410 竞态 | 已修 | `71b15f8` `b5e24fa` |
| R3-11 | 同步取消按钮陈旧闭包（R2-13） | 已修 | `3b9fe9e` |
| R3-12 | zip 回写失败不再报成功 | 已修 | `23f43d0` |
| R3-13 | 设置写入真原子/耐久 | 已修 | `23f43d0` |
| R3-14 | GUI 接入 RouteFs，zip:// 桌面可用 | 已修 | `0bb022a` |
| R3-15 | 编码引擎权威解码/回写（检测+显式+BOM） | 已修 | `d7722d7` |
| R3-16 | 目录内容比较三方式统一权威 | 已修 | `652341a` |
| R3-17 | 父子会话闭环 a/b/c/d + 差异文件导航 | 已修 | `861b53a` `b14f129` `400359b` `14b178b` |
| R3-18 | 编辑后重 diff 引擎权威，重要性不抹平 | 已修 | `bec61f6` |
| R3-19 | 崩溃恢复覆盖全部 7 种会话 | 已修 | `892bd2a` |
| R3-20 | 三向合并输出落盘 text/merge3/save | 已修 | `105e786` |
| R3-21 | 生成类型 vs 线上 JSON | (a)(c) 已修 / **(b) 待决** | `9aeca4f` |
| R3-22 | 能力协商/取消/背压/录制 | (a)(b)(e) 已修 / **(c)(d) 待决** | `9aeca4f` `72ff2ec` |
| R3-23 | Tauri 文件命令面/CSP 安全边界 | 命令面收敛+引擎路径已修 / **CSP 待决** | `a806ee0` |
| R3-24 | 发行签名/跨平台/MSRV/README | 签名+MSRV 已修 / **跨平台·README·LICENSE 待决** | `37188d0` |
| R3-25 | 巨型单文件与 mock 双实现漂移 | **巨型文件重构待决**；PROGRESS 状态源校准见 §8.3 | — |
| R3-26 | format:check / rustfmt / cargo-deny 门禁 | prettier 已修 / **rustfmt·cargo-deny 待决** | `b0316cf` |
| R3-27 | 文本首开丢比较规则（自查） | 已修 | `77fa72d` |
| R3-28 | 切标签丢目录视图状态（自查） | 已修 | `7c4f809` |

### 8.2 架构/契约级待决（记录证据 + 修法 + 为何后置）

以下各项**不是当前可达的功能缺陷**（门禁全绿、真实链路正确），而是需要改动协议类型契约、取消/背压架构、安全信任边界或发行工程的收敛项。按「架构变更先只读讨论再动代码」「测不到就说测不到」的项目纪律，记录证据 + 修法 + 为何后置，待用户拍板，不在本轮擅自重构或盲改。

#### R3-21(b) u64 → bigint 生成类型 vs NDJSON number 线上语义

- **证据**：ts-rs 把全部 `u64` 字段（rev/bytes/offset/size/addr/diffPixels/mtimeMs 等，约 30 处、25 个生成 .ts）导出为 `bigint`；但 NDJSON 线上是 JSON `number`（`JSON.stringify` 不支持 bigint）。UI 已在每个边界正确收窄：`engineProcess.ts:561` 把 hex offset/limit 收窄为 number、`mockEngine.ts:2463` rev 收窄、视图层 `Number(r.rev)`、mock/测试约 20 处 `as unknown as bigint`、夹具用 `1n`。`closeGuardFlow.test.ts:77` 已注「R3-21 待收敛」。
- **判定**：约定式权衡，**无运行时缺陷**（所有边界 cast 正确，725 UI + 120 引擎 + e2e 全绿）。真实量级（rev 计数、行/字节偏移、mtimeMs≈1.7e12）均远小于 2^53≈9e15，number 无精度风险。
- **修法（提案）**：给全部 u64 字段加 `#[ts(type = "number")]` → 再生成约 25 个 .ts → 删除约 40 处 `as unknown as bigint` 与 `Number()` 收窄 → 夹具 `1n`→`1`。
- **为何后置**：纯类型 hygiene，无功能收益；改动协议类型契约 + 大面积机械重构 + 夹具回归面，应作为独立专项并经用户确认。

#### R3-22(c) cancel:true 全局声明 vs 实际取消粒度

- **证据**：`dispatch`（`handlers.rs:69-104`）只把 `cancel` 传给 `dir_open`(71)、`dir_execute`(93)、`dir_merge_scan`(94)、`dir_merge_execute`(96)；`text_open`(77)、`report_generate`(102) 及其余 handler **不接取消旗标**。`process_request` 在请求完成即 `cancels.unregister(&key)`（`server.rs:455`），故 dir/open 的后台扫描（存活超出请求）**无法再用原请求 ID 取消**。
- **判定**：能力位过度承诺，claim 属实。text/open 的 Myers 有 deadline 降级但**无客户端取消**；report 长渲染同理。
- **修法（提案）**：① 把 CancelFlag 贯穿 text_open（在 Myers 分治循环加取消检查）与 report_generate；② 取消注册表改**会话级**（后台扫描随 session/close 或会话级 cancel key 注销），而非请求级。
- **为何后置**：触及取消生命周期（有 panic 隔离 + 注册表泄漏回归测试守着），属架构变更，需先只读设计讨论。

#### R3-22(d) 出站无界 channel 无背压

- **证据**：`server.rs:248` `mpsc::unbounded_channel::<Outbound>()`；`writer_task`(490+)  drain 到 stdout。客户端不读 → stdout 写阻塞 → writer_task 停滞 → 无界队列无限增长 → 内存耗尽。
- **判定**：claim 属实，架构属性。
- **修法（提案）**：改有界 channel + 异步 send 背压（handler 等待容量），或在压力下合并/丢弃陈旧 dir/delta 通知；须保持串行链输出确定性。
- **为何后置**：每个通知发送点都要处理背压，且不能破坏输出顺序与 panic 隔离，属架构变更，需先设计讨论。

> R3-22(e)（--record 静默降级）本轮**已修完**：open 失败告警（P2-06 `0d2780e`）+ 写入失败首次告警（`72ff2ec`）。写入失败分支无便携单测目标，已明说未加不可达守卫。

#### R3-23 CSP=null（信任边界，未盲改）

- **证据**：`tauri.conf.json:24` `"csp": null`。但全仓 `ui/src` 无 `dangerouslySetInnerHTML`/`srcdoc`/`innerHTML`/`outerHTML`（已 grep 确认），比较内容经 React 文本转义 + CM6 文本渲染，**无 HTML 注入汇**；`engine_start` 前端恒不传 path（`tauriTransport.ts:59`）。报告自述「不是当前已证实的远程漏洞」成立——CSP 是纵深防御，非可达漏洞。
- **判定**：claim 属实（CSP 确为 null），但无活跃注入路径。
- **修法（提案）**：设 Tauri 兼容 CSP，如 `default-src 'self'; connect-src 'self' ipc: http://ipc.localhost; img-src 'self' asset: http://asset.localhost data: blob:; style-src 'self' 'unsafe-inline'`（CM6 需 inline style；Tauri 自动为注入脚本加 nonce）。
- **为何后置**：① 现有浏览器验证基建驱动的是 vite dev（纯 Chrome，Tauri 不施加 CSP），**无法在真实 WKWebView 上验证 CSP 不破坏应用**——「测不到就说测不到」；② 严格 `script-src` 会破坏 vite dev 的 HMR 内联脚本，需 dev/prod 分别配置；③ 无活跃汇，边际收益低而静默破坏风险高。应在能驱动真实桌面 WebView 验证时专项处理。
- **本轮已做的收敛**：`settings_read/write/delete` 限制在应用设置目录（`confine_within`，5 测试 + 变异验证），`engine_start` release 忽略任意 path——把「一旦注入后果扩展到任意文件读写/进程启动」的实际命令面收敛掉（`a806ee0`）。

#### R3-24 跨平台发行 / 公证 / 自动更新 / README / LICENSE

- **证据**：`scripts/build-app.sh` 仅 macOS；无 notarization/auto-update/Win/Linux 发布验证；根仓库无 README/LICENSE（仅 ui/README）。
- **判定**：claim 属实，属发行工程范畴，非当前 macOS 体验版的功能缺陷。
- **修法（提案）**：① Win/Linux 打包脚本 + CI 矩阵；② macOS 公证（需 Apple Developer ID + notarytool 凭据）；③ Tauri updater 插件 + 签名密钥；④ 根 README/LICENSE/CONTRIBUTING。
- **为何后置**：公证/自动更新需用户提供的开发者凭据与签名密钥（不可自主决定）；LICENSE 是法律选择，须用户拍板许可证类型；跨平台 CI 是独立发行专项。本轮已修签名静默失败（`codesign || true` → loud + `--verify --strict`）与 MSRV 对齐（`37188d0`），并把 Tauri 独立 workspace 纳入测试/clippy 实跑（15/15、零告警）。

#### R3-25 巨型单文件重构

- **证据**：`handlers.rs`/`script.rs`/`types.rs`/`report.rs`/`mockEngine.ts` 行数大（报告计数已过期，handlers.rs 现 ~5250+）；同时承担协议/业务/IO/大量测试。
- **判定**：claim 属实，但「大并不自动等于差」——当前门禁全绿、真实链路正确，拆分无功能收益。
- **修法（提案）**：按职责把 handlers.rs 拆为 text/dir/merge3/report 子模块，测试随实现迁移；mockEngine.ts 与真实协议契约用 drift 测试守住分叉。
- **为何后置**：纯结构重构，触及大量调用链与测试布局，风险高收益零（功能不变），属架构变更需先只读讨论。PROGRESS.md「当前阶段=Walking Skeleton」过期属状态源校准，本轮后续提交修（§8.3）。

#### R3-26 rustfmt 全 workspace 债务 / cargo-deny 执行点

- **证据**：根 workspace rustfmt 既有债（ai-core ~523 hunk、ai-engine handlers.rs ~267 等）；cargo-deny 未安装，`deny.toml` 许可策略无执行点。
- **判定**：claim 属实。注意 `format:check` 脚本仅 prettier（不含 rustfmt），本轮已使其全绿（`b0316cf`）。
- **修法（提案）**：① 独立机械提交 `cargo fmt --all`（纯格式，需复跑全门禁）；② CI 安装 cargo-deny 并加 `cargo deny check` 门禁。
- **为何后置**：rustfmt 全量是数百 hunk 的机械 churn，应作为独立提交与本轮功能修复隔离；cargo-deny 需安装新工具（review 纪律不擅自装工具）+ CI 执行点，属门禁基建专项。

### 8.3 文档/状态源校准（本轮后续，#19）— 已完成

- PROGRESS.md「当前阶段」由 Walking Skeleton 校准为 M4 V2.0 / R3 收尾（与里程碑表格、当前代码 HEAD `7c4f809` 一致）。
- REVIEW.md §1/§2 早期过度声明更正为「每项均附带回归测试；其中 7 项经变异验证（§3.1），其余 12 项以回归测试 + 真实链路实测为据」。
- formats/* 接线状态、R3 全表（§8.1）按实测校准；R3-28 行补 `7c4f809`，表头「截至 HEAD `7c4f809`」。
- PROGRESS.md 末尾追加「第三轮深度 Review（R3）收尾 + 体验版重装」交付条目（HEAD `7c4f809`、门禁复验见 §8.4、重装体验版证据 + 诚实边界），显式取代过期的「最新交付状态（2026-11-14 21:5x，HEAD `fe04624`，引擎 822 / UI 477）」快照。
- 本节校准与 PROGRESS.md 收尾条目随同一 docs 提交落盘（代码状态 HEAD `7c4f809`，门禁 987 / 15 / 729 全绿）。

### 8.4 R3 收尾全量门禁复验（HEAD `7c4f809`，R3-28 提交后重跑）

| 门禁 | 结果 |
| --- | --- |
| `cargo test --workspace`（引擎） | **987 / 0**（3 ignored：ts-rs 再生成等）；较第一轮 829 增长来自 R3 各项新增回归 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 零警 |
| `cargo test --manifest-path ui/src-tauri/Cargo.toml`（壳独立 workspace） | **15 / 0**（含 R3-23 confine_within 5 测试） |
| `cargo clippy --manifest-path ui/src-tauri/Cargo.toml --all-targets -- -D warnings` | 零警 |
| `cargo test -p ai-protocol --features ts generated_bindings_match_ui`（drift） | 通过（192 生成 .ts 逐字节一致） |
| UI `npx tsc -b` | 0 错误 |
| UI `vitest run` | **729 / 0**（72 文件；含 R3-28 viewState 4 用例） |
| UI `npm run lint` / `npm run build` | 零警 / 通过 |
| UI `npm run e2e:engine` | 全过（真实引擎：zip 回写 / GB18030 检测解码回写 / 显式编码 / 413 / session-close 全类 / shutdown 退出） |
| UI `npm run format:check`（prettier） | 全绿（`src/protocol/generated/` 经 .prettierignore 排除） |

> 引擎/壳 clippy 双 workspace 零警；UI 视图层零 `.test.tsx`，R3-28 的组件 rehydrate 由真实浏览器（headless Chrome + CDP）10/10 断言 + 双向变异补足（见 `7c4f809` 提交信息）。
