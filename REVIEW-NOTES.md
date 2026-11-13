# ai-compare 深度 Review 工作笔记（非交付物）

> 用途：review 过程的工作笔记与断点记录。最终交付物是同目录 `REVIEW.md`，本文件在交付完成后可删除。
> 边界：**不修改任何业务代码**；只允许新增/更新 `REVIEW.md` 与本文件。

## 0. 任务

对 `/Users/ola/drax-toolbox/ai-compare` 做深度 code review，产出中文评审文档 `REVIEW.md`。

## 1. 续跑信息

- 定时任务：`cron once`，cronId `6d8690da-5e15-49db-966a-eb11e4f313f7`
- 绑定会话：`mvs_093000f8034548979f7bba53304cd407`（同会话续跑，保留上下文）
- 续跑时刻：2026-11-14 03:09 左右（runAtMs 1794596944440）
- 取消方式：`mcp_invoke cron_delete {cron_id}`

## 2. 仓库地图（已勘察）

Rust workspace（根 `Cargo.toml` members）：

| crate | 路径 | 职责 | LOC |
| --- | --- | --- | --- |
| ai-protocol | engine/ai-protocol | 进程间契约类型 + ts-rs 导出 | ~1500 |
| ai-core | engine/ai-core | diff 纯算法（无 IO） | ~3600 |
| ai-vfs | engine/ai-vfs | VfsProvider 抽象 / local / zip / crc32 | ~1100 |
| ai-dir | engine/ai-dir | 目录扫描/对齐/过滤/缓存/同步 | ~4700 |
| ai-engine | engine/ai-engine | JSON-RPC over stdio 服务端 + 编排 | ~1600 |
| aic | cli/aic | CLI（进程内链接引擎库） | ~700 |
| ai-compare-tests | tests/ | golden / replay / 对拍 / PB 基准 | ~800 |

UI：`ui/` = Tauri 2 壳（`ui/src-tauri`，**独立 workspace**）+ React 18 + TS + zustand + CodeMirror 6 + TanStack Virtual，约 7k 行 TS/TSX。
关键目录：`ui/src/protocol`（客户端/传输/jsonrpc）、`ui/src/session`（持久化/作用域/服务）、`ui/src/shell`（窗口/标签/快捷键/设置）、`ui/src/views`（home/text-compare/folder-compare）、`ui/src/ipc`（tauriTransport/engineProcess/reconnect/mockEngine）。

文档：`docs/01-需求规格说明书.md`(1548 行) / `02-开源竞品与技术调研报告.md`(571) / `03-架构设计文档.md`(1033, 10 ADR) / `04-开发计划.md`(584)。

## 3. 环境事实（已验证）

- cargo/rustc 1.100.0、node v26.8.1、pnpm 12.18.0 均可用
- `cargo` 在 `$HOME/.cargo/bin`，执行前需 `source "$HOME/.cargo/env"`
- `.cargo/config.toml`：crates.io → rsproxy.cn sparse 镜像
- `rust-toolchain.toml`：仅钉 `channel = "stable"`（**未钉版本**）
- `Cargo.toml` `rust-version = "1.80"`；`ui/src-tauri/Cargo.toml` 写 `rust-version = "1.77.2"`（**两处 MSRV 不一致**）
- `ui/src-tauri/Cargo.toml` 含空 `[workspace]` 表 → Tauri 壳**不在根 workspace 内**，根 `cargo test`/`cargo clippy` **不覆盖壳层**
- `deny.toml`：许可白名单 MIT/Apache/BSD/ISC/Zlib/CC0/BSL/Unicode，GPL/AGPL/LGPL/MPL 明确 deny；`[bans] multiple-versions = "warn"`；`[sources] unknown-registry = "deny"`，`allow-registry = ["https://github.com/rust-lang/crates.io-index"]`
- **无 `.github/` 目录 → 无 CI**
- 依赖：ai-core→imara-diff 0.2（Histogram）；ai-dir→rusqlite 0.32 bundled（SQLite 缓存，ADR-004）；ai-protocol→ts-rs 10（feature `ts`，default 开启）
- `tests/algodiff_matrix/generate_fixtures.py` —— 语料生成器是 **Python**，仓库未声明 Python 为工具链依赖

## 4. Git 状态（重要）

- 分支 `main`，最新提交 `b2b9f54`（M2-巡检 02:00）
- **工作树有 31 个文件未提交**：20 个 ` M` + 11 个 `??`，`git diff --stat` = **+2017 / -205**
- 未提交主体是 P2（目录引擎完备化）产出：`engine/ai-dir/src/{align,cache,filter,quick,session,sync}.rs`、`engine/ai-vfs/src/{crc32,zip}.rs`、`engine/ai-engine/src/pump.rs`，以及 `ai-protocol/types.rs`(+534)、`ai-engine/handlers.rs`(+541)
- `PROGRESS.md` 开发约定要求「开发 agent 不自行 git commit」，故这批 2017 行**长期悬空未入库**，存在丢失风险

## 5. 已确认发现（证据充分，可直接进 REVIEW.md）

### F1 — PROGRESS.md 表格结构被截断（P1，流程可靠性）
`PROGRESS.md:49-58`：「运行中任务登记」表被 `### M2 断点`（49 行）+ 编号列表 1–4（51–54 行）打断，第 **55 行**又出现孤儿表格行 `| 2026-11-13 21:3x | Agent「P0 UI 线」…`，直到 58 行才是另一截表格尾部。Markdown 渲染后两截都退化为普通文本，**状态源文件不可机读**。修复：表格收拢、拆分为独立小节。

### F2 — PROGRESS.md「当前阶段与断点」严重滞后（P1）
`PROGRESS.md:29` 仍写「当前：M1 Walking Skeleton（docs/04 P0 阶段）双线并行开发中」，但 `git log` 显示 M1 已闭环（a0801ca）、P1 已完成（c784160）、P4 已完成（7d5df90）、P2 自验中。里程碑总览（第 21–25 行）也未反映 P1/P4 已完成。**唯一状态源文件与现实脱节**，会误导后续 agent（该项目是 agent 自主开发，文档即控制面）。

### F3 — 状态源文件未登记「工作树脏」这一最高风险（P0/P1）
`PROGRESS.md:46` P2 行仍标「运行中」，但 P2 实际已接近收尾（最新提交说明「语料生成器收尾」）。此时工作树有 2017 行未入库改动，文档未把它登记为风险项。自主开发流程里「agent 不 commit + 主会话未及时提交」= 崩溃即丢失。修复：把「未提交改动清单」作为固定登记项，并明确每轮看护必须提交。

### F4 — 无 CI（P1）
无 `.github/workflows`、无 Makefile、无 justfile、无 `.gitlab-ci.yml`。质量门禁（cargo test / clippy / cargo-deny / pnpm test / lint / e2e:engine）全部依赖 agent 在本地会话里手动跑并自述结果。`deny.toml` 声明的许可零容忍策略**没有任何自动化执行点**，GPL 依赖可静默引入。`Cargo.lock` 已提交（好），但 `Cargo.lock` 的 diff 显示 P2 新增了 rusqlite/imara-diff 等依赖——恰好是许可敏感项。

### F5 — 工具链不可复现（P2）
`rust-toolchain.toml` 只写 `channel = "stable"`，无版本号；`Cargo.toml` 声明 `rust-version = "1.80"`（MSRV），而实际依赖 `ts-rs 10` / `thiserror 2` / `rusqlite 0.32` 的真实 MSRV 未被验证。stable 漂移即可能构建失败，且本地/CI/发布环境行为不一致。

### F6 — MSRV 声明不一致 + 壳层脱离 workspace 覆盖（P2）
根 `Cargo.toml` `rust-version = "1.80"`，`ui/src-tauri/Cargo.toml` `rust-version = "1.77.2"`。同时 `ui/src-tauri` 自带空 `[workspace]`，根 `cargo clippy --all-targets` **不会** lint 壳层 367 行 Rust。壳层承担 spawn 引擎进程 / stdio 透传，是进程边界风险最高的地方，却覆盖最弱。

### F7 — cargo-deny 与 rsproxy 镜像的潜在冲突（P2，待实测）
`deny.toml` `[sources] unknown-registry = "deny"` 且 `allow-registry` 只含 crates.io 官方 index；而 `.cargo/config.toml` 把 crates-io **替换**为 `rsproxy-sparse`（`sparse+https://rsproxy.cn/index/`）。cargo-deny 在 source replacement 生效时通常按被替换后的 source 计算，rsproxy 不在 allow 列表 → 极可能 `unknown-registry` fail。需实跑 `cargo deny check` 验证（该二进制当前可能未安装）。

## 6. 待查清单（续跑时按此推进）

### A. ai-core（算法正确性，最高优先级）
文件：`myers.rs`(439) `patience.rs`(243) `histogram.rs`(154) `inline.rs`(429) `incremental.rs`(678) `progressive.rs`(517) `pipeline.rs`(462) `keys.rs`(532) `lines.rs`(349) `manual.rs`(260) `options.rs`(260) `sections.rs`(323)
查点：
1. Myers O(ND) 的 D 路径回溯是否正确处理多解；`similar`/`imara-diff` 依赖的实际使用边界
2. Patience 的 LIS 手写实现 vs `patience` crate 的差异；锚点回退 Myers 的正确性
3. Histogram 直通 imara-diff 的 Token 序列构造是否与行级语义一致
4. 重要性双键（keys.rs 532 行）是否对称、是否有分桶哈希碰撞风险
5. 增量重 diff（incremental.rs 678 行）窗口锚定 + 边界吸收的**正确性证明缺失**：编辑落在锚点边界时是否可能漏 diff/产生错位
6. 行内 diff 超时降级路径：降级后是否静默产出**错误结果**而非标记失败（这属于正确性缺陷，不能只是降级）
7. DMP（diff-match-patch）清理逻辑与 O(ND) 的交互
8. progressive.rs 的流式输出是否可能丢块/乱序
9. 边界：空输入、超长行（无换行）、BOM、CRLF/LF 混合、非法 UTF-8、大文件内存占用

### B. ai-dir（目录对比与同步状态机）
文件：`session.rs`(1619) `align.rs`(362) `compare.rs`(219) `filter.rs`(721) `quick.rs`(247) `cache.rs`(237) `sync.rs`(318) `lib.rs`(372)
查点：
1. 名称对齐（align.rs）策略与 BC 语义一致性；同名不同内容/大小相同内容不同的判定
2. 快速测试全集（quick.rs）的短路条件是否会漏判（如 mtime 相同但内容不同）
3. 过滤器引擎侧求值（filter.rs 721 行）：通配符/正则/大小写/排除优先级的组合语义，是否与 UI 侧重复实现导致双份规则漂移
4. SQLite 缓存（cache.rs）：key 设计、失效策略、mtime/大小变更检测、并发访问（多线程 rusqlite 连接）、缓存陈旧导致的**误判相同**
5. 同步状态机（sync.rs 318 行）：计划→执行的状态迁移、失败回滚、幂等性、部分失败后的重入
6. session.rs 1619 行 —— 重点看状态机正确性、取消（CancelFlag）语义、内存驻留 10 万项的可行性
7. zip 预计算 CRC 与本地 crc32（ai-vfs/crc32.rs）算法一致性

### C. ai-protocol / ai-engine / cli（协议边界与进程模型）
文件：`ai-protocol/types.rs`(1226) `error.rs` `ts_export.rs`(116)；`ai-engine/handlers.rs`(795) `server.rs`(343) `batch.rs`(130) `pump.rs`(96) `mapping.rs`(133) `log.rs`；`cli/aic/main.rs`(355) `lib.rs`(352)
查点：
1. JSON-RPC 2.0 合规性：id/错误对象/notification 语义、未知方法处理、错误码稳定性
2. stdio 传输：NDJSON 行缓冲、超长行（大 payload）、stdout 污染（任何 `println!` 混入协议流 → 解析崩溃）、stderr 是否污染
3. 并发/取消：`batch.rs` + `pump.rs` 的任务调度、有界并发、优先级（可视区优先）、取消的传播与竞态
4. `handlers.rs` +541 行未提交 —— 新增方法的错误处理是否一致、是否有 unwrap/expect/panic 在协议边界（panic 会杀死整个引擎进程）
5. ts-rs 导出与 `ui/src/protocol/generated/*.ts` 是否**真的同步**（P2 改过协议类型，UI 侧 generated 是 P4 提交的旧版？→ 需比对）
6. CLI 退出码契约 0/1/2 的定义与实际行为（diff 差异=1、错误=2）是否在所有子命令一致
7. 协议类型的字段可选性/默认值：新增字段对旧客户端的兼容性

### D. UI 侧（React/TS）
文件：`session/{storage,storageAdapter,model,service,scopes}.ts`、`shell/{store,shellState,sliceWindowsTabs,sliceSessionUi,viewRegistry,shortcuts,shortcutEngine,commands}.ts`、`protocol/{client,transport,jsonrpc,types}.ts`、`ipc/{tauriTransport,engineProcess,reconnect,mockEngine}.ts`、`views/**`
查点：
1. 会话持久化「原子写 + .bak 回滚」实现是否真的原子（临时文件 + rename？fsync？跨平台？）—— Windows 上 rename 覆盖行为差异
2. `storageAdapter` 的作用域合并（四层作用域）优先级实现与测试是否一致
3. 协议客户端的重连状态机（reconnect.ts）竞态：重连期间到达的通知是否丢弃、重连与请求在途的交互
4. `engineProcess.ts` 引擎进程崩溃/重启的清理（僵尸进程、stdio 半关闭）
5. `mockEngine.ts`（533 行）与真实引擎的协议一致性 —— mock 与真实行为分叉是 UI 测试的主要失真来源
6. `dirRowCache.ts` 删除未清 id 索引的 bug 已在 a0801ca 修复，检查是否还有同类 id 索引
7. React 层的性能与正确性：`@tanstack/react-virtual` 虚拟滚动的行高假设、CodeMirror 装饰器（cmDecorations.ts）与大 diff 的性能
8. 状态管理：zustand slices 是否有跨 slice 竞态；`store.ts` 是否过度集中
9. TypeScript strict 设置、`any` 使用、可访问性（对比工具的核心用户群含键盘操作）

### E. 测试与工程化
1. `tests/` 四个测试目标（golden / protocol_replay / algodiff_crosscheck / pb_bench）的**断言强度**：golden 快照是否会被「重新生成即通过」自证；对拍 19/21 的 2 个失败用例为何不算失败
2. `tests/algodiff_matrix/crosscheck_test.rs` 依赖 Python 预生成的 `fixtures.json`（478 行）—— 生成器 `generate_fixtures.py` 与 Rust 期望值的一致性如何保证；fixtures 是否进版本库且可复现
3. `tests/pb_bench.rs` 基准是否被 `cargo test` 路径误收（它注册在 `[[test]]` 下，默认会跑）
4. `ui/scripts/e2e-engine.mjs` 端到端脚本覆盖了哪些协议路径，漏了哪些（错误路径/取消/大 payload）
5. `ui` 侧 134 测试的分布与薄弱区（哪些模块无测试）
6. `deny.toml` 是否真被执行过；`cargo deny` 是否安装
7. `.gitignore` 覆盖度；`target/` 已忽略但 `ui/src-tauri/target` 是否也忽略（实测 status 未显示，应已忽略，需确认）

## 7. 续跑时先跑的命令

```bash
cd /Users/ola/drax-toolbox/ai-compare && source "$HOME/.cargo/env"
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p ai-compare-tests --release   # PB 基准
cd ui && pnpm test && pnpm lint && pnpm build && pnpm e2e:engine
```
UI 侧依赖需先 `pnpm install`（`ui/node_modules` 是否存在待确认）。

## 8. REVIEW.md 目标结构

1. 结论摘要（一句话结论 + 评分表 + 是否建议继续推进）
2. 项目与架构评估（分层、依赖方向、ADR 执行度、架构意图 vs 实现的差距）
3. 分级问题清单：P0 阻断 / P1 严重 / P2 改进 / P3 建议 —— 每条含 **位置(file:line) / 现象 / 根因 / 影响 / 修复建议**
4. 测试与工具链缺口（含覆盖率盲区）
5. 风险矩阵（概率 × 影响 × 可探测性）
6. 附录：命令输出摘录、状态源文件问题清单、后续建议排期

## 9. 已排除的误报（避免重复劳动）

- `tests/Cargo.toml` 把 ai-* 放在 `[dev-dependencies]` —— **不是 bug**：`tests/src/lib.rs` 只用 std + serde_json，ai-* 仅被 `[[test]]` 集成测试目标使用，dev-dependencies 合法。
- 根 workspace 不含 `ui/src-tauri` —— **有意的 ADR-001 壳/引擎解耦**，不是配置错误（但覆盖缺口本身仍是问题，见 F6）。
