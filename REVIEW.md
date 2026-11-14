# ai-compare 深度 Code Review 报告

> 评审时间：2026-11-14
> 评审范围：`engine/`（5 crate）+ `cli/` + `ui/`（Tauri2 壳 + React18 + TS）+ `tests/`
> 评审基线：引擎 822 测试 / UI 477 测试全绿的 `main`（HEAD `091a752`）
> 方法：静态走查 + 双路并行子代理深挖 + **变异验证**（逐条确认回归测试真能捕获缺陷）

---

## 1. 结论摘要

**一句话结论**：架构分层清晰、协议演进克制、测试密度可观（1299 个测试），整体质量高于同类自研项目；但**协议边界的健壮性**与**引擎会话生命周期**存在数个真实缺陷 —— 集中表现为「客户端断开后引擎进程挂死」「handler panic 导致请求永不应答」「JSON-RPC 2.0 不合规」「引擎会话只增不减」四类。

本次评审**核实并修复 19 项缺陷**（含 9 项引擎侧、9 项 UI 侧、1 项测试基建），**每项均附带回归测试并通过变异验证**（把修复回退，测试确实变红）。剩余 21 项按置信度与修复风险分级登记于 §4，留待后续批次决策。

| 维度 | 评分 | 说明 |
| --- | --- | --- |
| 架构与分层 | A | Rust 引擎 / Tauri 壳解耦清晰，ADR 有据可循，依赖方向无环 |
| 协议设计 | B | 方法集与载荷语义良好，但 JSON-RPC 2.0 合规性有 4 处缺口；ts-rs 导出与 serde 语义脱节 |
| 健壮性 | B- | panic 隔离缺失、进程退出路径依赖隐式时序、会话释放不完整 |
| 测试 | B+ | 1299 测试 + golden + 活体对拍（git/diff3/xxd/csv）；但缺 CI、断言强度不均 |
| 工程化 | C+ | 无 CI、工具链未钉版本、MSRV 不一致、壳层脱离 workspace 覆盖 |

**建议**：**继续推进**。本轮修复未改变任何既有协议语义（`RequestId` 增补 `null` 变体为向后兼容扩展），可安全合入。

---

## 2. 已修复缺陷（19 项，全部含回归测试 + 变异验证）

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
