# ai-compare 深度 Code Review

> 评审对象：`/Users/ola/drax-toolbox/ai-compare`（工作树含未提交的 P11 在制品改动，按磁盘现状评审）
> 评审时间：2026-11-14 10:2x–11:xx（CST）
> 评审方式：5 路并行只读审查（ai-core / ai-dir+ai-vfs / 协议+引擎+CLI / UI / 测试与工程化）+ 主会话实测质量门禁
> 边界声明：本次评审**不修改任何业务代码**；交付物仅为本文档。

---

## 1. 结论摘要

**一句话结论**：架构分层与测试资产是同类项目的高水位（五 crate 单向依赖执行度高、对拍/锚定/回放测试扎实、约 1100+ 断言面），但**核心承诺与实现存在名实不符**（panic 边界、原子写、崩溃自动重启三条均未真正落地），且缺陷在**输出层与状态机**集中爆发（report 下标错配、progressive 丢行、expand/worker 死锁、flat id 错路径）；工程治理（无 CI、状态源失真、P11 交付物悬空）是当前最紧迫的流程风险。**建议继续推进，但 P11 合库前必须先清偿 P0-2/P1-10 与快照重固化，本周内处理 P0-1 与治理三件套（入库/CI/PROGRESS.md）。**

**评分表**：

| 维度 | 评分 | 说明 |
| --- | --- | --- |
| 架构设计 | 8.5/10 | ADR 执行度高，引擎/UI 解耦是结构性资产；docs 与实现有小漂移 |
| 算法核心 | 8/10 | myers/patience/merge3/patch 扎实，对拍充分；但输出层（report/progressive）有 P1 缺陷 |
| 协议与进程模型 | 7.5/10 | serde 锚定/兼容测试充分；panic 边界名存实亡、错误 id 违规、无背压 |
| UI 工程 | 7/10 | 纯函数层测试扎实；崩溃重启/原子写两条承诺失真，flat id 数据安全隐患 |
| 测试资产 | 8/10 | 对拍归类有防呆设计、golden 配语义断言；e2e 仅 happy path，对拍可空转 |
| 工程化治理 | 4/10 | 无 CI、deny 零执行点、PROGRESS.md 失真、~1500 行交付物悬空 |
| 安全纵深 | 5.5/10 | 删除路径有护栏；写路径 symlink、zip 条目名、壳层 IPC 路径约束缺失 |
| **总体** | **7/10** | 向 1.0 推进的底子足够，先把名实不符与治理债清掉 |

**问题计数**：P0×2、P1×11（含 1 条并入 P2-2）、P2×29、P3×19；另有 4 项「待证实」。

**已实测的质量门禁现状（2026-11-14 10:3x，工作树含 P11 在制品）：**

| 门禁 | 结果 |
| --- | --- |
| `cargo test --workspace --no-fail-fast` | ❌ 7 失败 / 其余 582+ 通过（6 失败与 P11 在制品直接相关，见 §3-P0-2/§4） |
| `cargo clippy --workspace --all-targets -- -D warnings` | ❌ 1 错误（`tests/script_matrix/script_test.rs:52` 死代码） |
| `pnpm test`（ui/） | ✅ 383/383 |
| `cargo deny` | ⚠️ 未安装，deny.toml 无执行点 |

---

## 2. 项目与架构评估

### 2.1 项目概况

ai-compare 是 Beyond Compare 5 的功能复刻（产品名 ai-compare），由 agent 自主开发。体量：

| 层 | 内容 | 规模 |
| --- | --- | --- |
| 引擎 | Rust workspace 5 crate：ai-protocol（契约+ts-rs）/ ai-core（纯算法）/ ai-vfs / ai-dir / ai-engine（JSON-RPC stdio 服务端）+ cli/aic | ~13k 行 Rust |
| UI | Tauri 2 壳（独立 workspace）+ React 18 + TS + zustand + CodeMirror 6 + TanStack Virtual | ~7k 行 TS/TSX |
| 测试 | 引擎 ~700+ 单测/集成/对拍/golden/PB 基准 + UI 383 | 约 1100+ 断言面 |
| 文档 | docs/01 需求规格（163 编号需求）/ 02 调研 / 03 架构（10 ADR）/ 04 开发计划 | ~3700 行 |

### 2.2 架构评估

**分层与依赖方向（良好）**：引擎层严格无 UI 依赖（ADR-002），五 crate 单向依赖（ai-core ← ai-dir/ai-engine ← cli），进程边界 JSON-RPC over stdio（ADR-003）解耦 UI 与引擎，UI 可 mock 引擎独立测试。这一架构意图在实现中执行度高——引擎可全量单测、UI 可浏览器冒烟，是本项目最值得肯定的结构性资产。

**ADR 执行度**：

| ADR | 内容 | 执行度 |
| --- | --- | --- |
| 001 | Tauri 2 + React，Electron 保底 | ✅ 一致 |
| 002 | Rust headless 库 + 独立进程双形态 | ✅ aic 进程内链接 + ai-engine 独立进程 |
| 003 | JSON-RPC 2.0 over stdio NDJSON | ✅ 含向后兼容逐字节锚定测试 |
| 004 | 有界并行 + 惰性展开 + stat/hash 缓存 | ✅ PB-03 10 万项 1.54s 达标 |
| 005 | 版本化 JSON + 原子写 + .bak | 见 UI 审查章节 |
| 006 | regex + fancy-regex 双引擎 | 见格式/规则章节 |
| 007 | rclone sidecar 远程 VFS | 未到实施阶段（V2.0） |
| 008 | CodeMirror 6 编辑内核 | ✅ |
| 009 | Pro 特性开关而非架构分叉 | ✅ license.proUnlocked 能力位 |
| 010 | BC 行为兼容优先 | 大体遵守 |

**架构意图 vs 实现的差距**：
- 模块划分上 docs/03 规划了 `ai-diff3`、`ai-formats`、`ai-session` 独立 crate，实现中被并入 ai-core（merge3/formats/rules）与 UI 会话层——小 crate 合并是合理的体量裁剪，但 docs/03 未回写，文档与实现出现漂移。
- 质量门禁全部依赖本地人工/agent 自述（无 CI），与「可全量单测」的架构优势不匹配：测试资产充足但无自动化执行点。

---

## 3. 分级问题清单

> 分级：P0 阻断 / P1 严重 / P2 改进 / P3 建议。每条含【位置】【现象】【根因】【影响】【修复建议】。

### P0 — 阻断

#### P0-1 引擎崩溃后自动重启必失败（僵尸条目挡住 respawn）

- 【位置】`ui/src-tauri/src/lib.rs:106-113, 161`、`ui/src/ipc/engineProcess.ts:196-211`
- 【现象】`engine_start` 幂等逻辑仅判断 `guard.is_some()` 即返回旧 pid，不 `try_wait()` 探活；子进程退出后无人从 EngineState 摘除（lib.rs:161 注明「不做即时 waitpid」）。崩溃后重连走 engine_start 拿到死 pid → 写 stdin 失败 → 二次 close → `MAX_AUTO_RESTARTS=1` 已耗尽 → failed。僵尸进程驻留至应用退出。
- 【根因】崩溃后状态机持死进程条目，重启链路（reconnect.ts 状态机）没有活探。
- 【影响】「崩溃自动重启」核心承诺实际不可用。
- 【修复建议】engine_start 前 `child.try_wait()`，已退出则 kill/wait 后清空再 spawn；或前端崩溃路径先调 engine_stop。

#### P0-2 patch「已应用检测」回归（未提交 P11 改动引入，4 测试失败）

- 【位置】`engine/ai-core/src/patch.rs:533-547`（未提交 diff）；波及 `engine/ai-engine/src/handlers.rs:3094`、`tests/patch_matrix/patch_test.rs:450`、`tests/report_matrix/report_test.rs:392`
- 【现象】工作树实测 4 个测试失败：
  - `patch::tests::apply_already_applied_detected`（patch.rs:1164）：修改型 hunk（`-b`/`+B`）应用到已应用文件不再报「已应用」
  - `parser_tolerance_goldens`（patch_test.rs:450）：`err.contains("已应用")` 断言失败
  - `handlers::p9_tests::patch_apply_rpc_roundtrip`（handlers.rs:3094）：期望 `alreadyApplied` 实得 `failed`
  - `report_matrix::patch_layout_roundtrip` crlf_vs_lf_content 语料同类受累
- 【根因】未提交修复为消除「纯删除 hunk 误判已应用」，增加了 `has_delete` 抑制条件（patch.rs:541-546）。但**任何含删除行的 hunk**（即全部修改型 hunk）都被跳过检测——最常见的补丁形态失去幂等保护。注释自述目标是「纯删除 hunk（新内容仅为上下文）」，精确条件应为「hunk 无新增行」（`!h.lines.iter().any(|l| l.kind == PatchLineKind::Add)`），而非「含删除行」。
- 【影响】补丁幂等性（alreadyApplied 语义）在 CLI/RPC 双面失效；重复应用补丁将产出错误结果或报模糊错误。
- 【修复建议】把 `has_delete` 换成「无 Add 行」判定；保留 `new_expected_texts != expected_texts` 双条件；跑 `patch_matrix` + `report_matrix` + `ai-engine p9_tests` 三面回归。

### P1 — 严重

#### P1-1 panic 边界名存实亡，协议边界 expect 可致客户端挂死

- 【位置】`engine/ai-engine/src/lib.rs:15`（文档承诺「panic 被捕获转换为 InternalError 应答」）、`server.rs:265`（`let _ = p.await` 吞 JoinError）、`server.rs:315`、`handlers.rs:835/847/861`（`sec.left.expect("Change/Delete 必有左区间")` 等协议边界 expect）
- 【现象】全库 grep 无 `catch_unwind`。handlers 中 expect 一旦触发，spawn 任务死亡，串行链吞掉 JoinError，**该请求永远无应答，客户端挂死**；整个引擎进程还可能因 panic 策略不一致而行为不可预期。
- 【根因】只在 await 链上忽略 panic，未做应答兜底；文档承诺未落地。
- 【影响】协议边界单个请求缺陷 → 客户端（UI/CLI）无限等待；比直接 crash 更难探测。
- 【修复建议】`process_request` 外层 `AssertUnwindSafe + catch_unwind`（或按 JoinError 映射）回 `-32603`；消除 handlers 的 expect（改错误应答）。

#### P1-2 expand 与 worker 预登记冲突：子树无 worker 消费、超时目录永久卡死

- 【位置】`engine/ai-dir/src/session.rs:332, 752-782`；`engine/ai-engine/src/handlers.rs:414`
- 【现象】expand 先插入 in_flight 再入队 fg；`pop_item` 见 in_flight 即丢弃该项并减 pending——有 worker 时前台项无人处理，须待 worker 误判 pending=0 过早 finish 全部退出后走 Completed 兜底；30s 超时分支不处理也不清 in_flight，**该目录永久卡死**。
- 【根因】预登记语义与 pop_item 去重冲突。【影响】全量会话 expand 后 commit 进 bg 队列的子树无 worker 消费、永不扫描，phase/进度虚报完成（handlers.rs:414 也未补启 worker）。
- 【修复建议】pop_item 放行前台登记项或 expand 不预登记；超时分支补同步处理并清理 in_flight；补 expand-with-workers 集成测试。

#### P1-3 CaseFold/Pattern 对齐后右路径用左名拼接：非同名对齐读错文件

- 【位置】`engine/ai-dir/src/session.rs:478, 517-529`、`lib.rs:207, 226`
- 【现象】左右路径均用 `pair.name`（左名）拼 rel：右名不同（readme.txt、report.pdf）时内容比较读错路径；大小写敏感 FS 报 NotFound→整体失败（scan() 中 `?`）；右树恰有同名不同文件时误判 Same。
- 【根因】未按 `pair.right` 的真实名构造右路径。【影响】非同名对齐判定错误/扫描中断。
- 【修复建议】SidePaths 改用两侧真实文件名各自 join；补非同名对齐集成测试。

#### P1-4 写路径无 symlink 护栏：目标树预置符号链接可重定向写穿

- 【位置】`engine/ai-dir/src/execute.rs:404-455`、`engine/ai-vfs/src/local.rs:182-235`、`lib.rs:368`
- 【现象】copy_file/write/mkdir_all 均跟随目标侧符号链接（mkdir_all 甚至把 symlink 当成功）；中间目录 link→树外时复制写穿覆盖任意可写文件。护栏（sanitize_rel+不跟随）**仅覆盖删除**。
- 【影响】镜像/合并可被目标树预置符号链接重定向毁数据（安全纵深缺口）。
- 【修复建议】写前 lstat 目标链拒绝跟随，或落盘前 canonicalize 校验仍在根内。

#### P1-5 PatternAligner 首命中互配错配 + limit_to_dir 静默失效

- 【位置】`engine/ai-dir/src/align.rs:215-235`、`191-197`
- 【现象】每条规则仅取左右首个未占用命中互配（非按名干配对）：a.gif、b.gif ↔ b.png、a.png 会 a↔b 错配，与 BC「同名干配对」不符；`limit_to_dir/dir_limited_rule` 全库无消费方，目录限定静默失效。
- 【修复建议】按掩码剔除段后的名干配对；接线或删除 limit_to_dir。

#### P1-2b P11 交付物大面积悬空未入库（丢失风险）→ 见 P2-2

（并入 P2-2 处置，等级按「可能丢工作」视为 P1。）

#### P1-6 settings_write「原子写」名实不符：无 fsync、失败路径丢主文件、tmp 名可冲突

- 【位置】`ui/src-tauri/src/lib.rs:299-331`、`ui/src/session/storage.ts:8-9`
- 【现象】`rename(主→bak)` 后主文件已不存在，第二步 `rename(tmp→主)` 失败时注释「目标与 .bak 均保持旧值」不成立（目标只剩 .bak）；全程无 fsync，与 storage.ts:8「fsync 由后端负责」矛盾；tmp 后缀仅 pid，同进程并发写同文件共用 tmp 路径互踩。
- 【根因】实现按「移动旧文件做备份」而非「复制备份 + tmp + fsync + rename」；tmp 命名缺随机位。
- 【影响】断电/写失败可丢最新写入甚至主文件短暂缺失；并发保存损坏。recoverWithBackup 能兜「主缺失」，但 ADR-005 不变量承诺失真。
- 【修复建议】tmp 加随机后缀并 `File::sync_all`+目录 fsync；备份用 copy 而非 rename。

#### P1-7 closeWindow 对脏标签不走保存确认，直接销毁窗口丢数据

- 【位置】`ui/src/shell/sliceWindowsTabs.ts:73-87, 283-297`
- 【现象】closeWindow 逐标签 closeTab（脏命名会话仅 set closeGuard 即 return），随后无条件删窗；resolveCloseGuard 再 findTab 已找不到。closeOthers 完全绕过脏检查。
- 【影响】关闭窗口静默丢失未保存会话修改（FR-SES-001 失守）；或孤儿对话框。
- 【修复建议】closeWindow/closeOthers 检测 dirty 改走 closeGuard 队列，确认后再删窗。

#### P1-8 平铺模式行 id 不再是 rel 路径，文件操作/子会话打开指向错误文件

- 【位置】`ui/src/ipc/mockEngine.ts:904`（`id = name.toLowerCase()` 或 `name#i`）、`views/folder-compare/dirOps.ts:62`（`path: id`）、`shell/sliceWindowsTabs.ts:226-227`（`joinPath(left, row.id)`）
- 【现象】树形 id=rel 路径，flat 派生行 id=小写名+#i 消歧；而 buildOpsForCommand、openTextChild、expandDir 均把 id 当 rel 路径用。REVIEW-NOTES 提示的「同类 id 索引问题」在此仍有残留。
- 【影响】flat 模式下复制/删除/双开作用到错误路径（如 `readme.md#0`），**属数据安全问题**；真实引擎同形协议下同样中招。
- 【修复建议】DirRow 增加 `relPath` 字段（flat 行保留原 rel），操作/打开一律用 relPath。

#### P1-9 doConnect 与 handleTransportCrash 竞态：过期 client 覆盖 null

- 【位置】`ui/src/ipc/engineProcess.ts:154-156` vs `188-190`
- 【现象】`await c.ready` 与 `client = c` 之间若 transport 崩溃，crash 处理先置 `client=null`，随后 doConnect 把已关闭的 `c` 写回 `client` 并 resolve 给调用方。
- 【影响】此后 getEngineClient 持续返回死 client，所有请求 reject CLOSED，直至再次崩溃；通知断流。
- 【修复建议】doConnect 用连接代际（generation）计数，赋值前校验代际仍当前。

#### P1-10 report.rs context 补丁用左行下标索引 right_lines（可稳定触发 panic）

- 【位置】`engine/ai-core/src/report.rs:1109`（`build_patch_script:900-902` 同源）
- 【现象】`PatchEntry::Same(i)` 只携带左下标，渲染右半段时 `snap.right_lines[*i]`。任何前置存在插入/删除的输入都输出错误上下文行；右文件比左短时**直接越界 panic**（如左 `"a\nb\n"`、右 `"b\n"`）。
- 【根因】Same 未携带右侧对应下标；Equal 段左右区间起点通常不同。
- 【影响】P11 报告引擎 patch 布局输出错误/崩溃（未提交代码，合库前必修）。
- 【修复建议】Same 同时记录右下标，或按 section 右区间换算；补「右短于左」用例。

#### P1-11 progressive 边界吸收改写相邻 section 但 Replace 事件未覆盖（流式丢行）

- 【位置】`engine/ai-core/src/progressive.rs:224-260, 291`
- 【现象】块内边缘 Equal 被并入 `sections[prev_idx]/[next_idx]`（就地扩展区间），事件 `span=(head_id,head_id)`、`with` 仅含剩余精化段。按 `stream_shape_matches_one_shot` 重放契约消费事件会**丢行**（吸收段从视图消失），流式 UI 与终态不一致。
- 【根因】吸收结果未纳入 span/with；`coarse_then_refine_events` 只查引擎态、未做事件重放。
- 【修复建议】span 扩展到被吸收邻段（参考 incremental.rs:342-347），或对邻段补发 Replace；补吸收场景的事件重放对拍测试。

### P2 — 改进

#### P2-1 无 CI，deny.toml 零执行点

- 【位置】全树无 `.github/`、Makefile、justfile；`deny.toml:1-2`
- 【现象】cargo test / clippy / deny / pnpm test / lint / e2e:engine 全部依赖 agent 本地自述；`cargo-deny` 未安装，GPL 零容忍（deny.toml 明确 deny GPL/AGPL/LGPL/MPL）没有任何自动化执行点。
- 【根因】从未搭建 CI。【影响】质量与许可门禁可静默失效；agent 自述结果无法独立核验。
- 【修复建议】补 GitHub Actions：`cargo test --workspace` + `clippy -D warnings` + `cargo deny check` + `pnpm test/lint/build` + e2e 冒烟。

#### P2-2 P11 产出大面积悬空未入库（丢失风险）

- 【位置】git status 31 项：`git diff` +930/-17（handlers.rs +422、types.rs +282、mapping.rs +124），未跟踪 ≥600KB（`engine/ai-core/src/report.rs` 116K、`cli/aic/src/script.rs` 120K、`tests/golden/snapshots/report/` 304K、report_matrix/script_matrix、9 个 generated TS）
- 【现象】PROGRESS.md:81「开发 agent 不自行 git commit」约定下，看护任务只提交 PROGRESS.md（eaacaa3 等），代码长期悬空；P11 仍标「运行中」。
- 【影响】会话/机器异常即丢约 1500 行交付物 + 304K golden 快照。**建议立即按 scope 提交或打包存档**。
- 【修复建议】P11 收口后主会话立即验收提交；约定看护任务每轮巡检必须核对工作树脏状态并登记风险。

#### P2-3 PROGRESS.md 状态源失效（重演历史问题）

- 【位置】PROGRESS.md:33（「当前」仍写 M1 Walking Skeleton）、:65-74（「运行中任务登记」表被 `### M3 断点` 切断，71-74 行成孤儿表格行，Markdown 渲染退化）、:65（断点列表 P3/P5 均已完成未销账）
- 【现象】唯一状态源与现实脱节：实际已到 M3/P11；表格结构第二次被截断（REVIEW-NOTES F1 同类）。
- 【影响】该文档是 agent 接手/看护的控制面，失真直接导致误判进度。
- 【修复建议】收拢表格为连续块；「当前阶段」改写为 M3/P11；已完成断点销账。建议引入 lint 脚本校验表格行连续性。

#### P2-4 golden/replay 快照待重固化（P11 在制品）

- 【位置】`tests/golden/snapshots/`（3 个快照）、`tests/protocol_fixtures/session1.record.ndjson`
- 【现象】P11 新增 `capabilities.reports=true` 能力位，golden 3 处 + replay 1 处快照失配（工作树实测）。属有意变更，`UPDATE_GOLDEN=1` 重固化即可，但需人工核对 diff 仅含该行。
- 【修复建议】重固化并把 diff 逐行列入验收记录（项目已有此纪律）。

#### P2-5 clippy 门禁失败（P11 在制品）

- 【位置】`tests/script_matrix/script_test.rs:52`（`Sandbox::read` 从未使用）
- 【现象】`-D warnings` 下 `-D dead-code` 触发，`ai-compare-tests` 编译失败，clippy 门禁整体红。
- 【修复建议】删除或使用该方法；clippy 应进 CI。

#### P2-6 merge3 对拍可零比较空转

- 【位置】`tests/merge3_matrix/merge3_test.rs:166-167, 373-375, 388-389`
- 【现象】diff3/git 工具缺失时 `run_tool` 返 None → verdict="skipped"，断言仅拒 "diff"，全部跳过也通过；ambiguous 语料（:324）差异整类豁免、无性质校验。对照 `patch_test.rs:227`、`hex_test.rs:21` 缺工具即硬失败。
- 【影响】对拍可能静默变成零比较，掩盖真回归。
- 【修复建议】断言 same≥N 或工具缺失即 fail；ambiguous 语料补重放/编辑量守恒性质校验。

#### P2-7 Python 工具链依赖未声明、fixtures 一致性无保障

- 【位置】`tests/algodiff_matrix/generate_fixtures.py`、`tests/table_matrix/generate_fixtures.py`；PROGRESS.md:85 工具链仅声明 node/pnpm/rustc
- 【现象】两套 fixtures 由 python3 预生成（已入库），一致性仅靠生成时 difflib==git 自检；无测试验证 fixtures.json 与生成器输出逐字节一致。
- 【修复建议】文档声明 python3+git 依赖；CI 跑生成器并 diff 比对。

#### P2-8 e2e-engine.mjs 仅覆盖 happy path

- 【位置】`ui/scripts/e2e-engine.mjs:135-200`
- 【现象】覆盖 initialize/dir/open/dir/delta/text/open/text/sections/session/close/shutdown 共 6 方法（协议 ~30 个）；漏错误路径、`$/cancel`、大 payload 分页 hasMore=true、merge3/report/formats/rules/dir execute。
- 【修复建议】补取消、错误码、万行分页场景。

#### P2-9 工具链不可复现 + 壳层 lint 覆盖缺口

- 【位置】`rust-toolchain.toml:3` 仅 `channel="stable"`（未钉版本）；根 `Cargo.toml:19` rust-version=1.80 vs `ui/src-tauri/Cargo.toml:7` 1.77.2（MSRV 不一致）；`ui/src-tauri/Cargo.toml:11` 独立 workspace → 根 clippy 不覆盖壳层 367 行 Rust（spawn 引擎进程/stdio 透传，恰是进程边界风险最高处）。
- 【修复建议】钉具体工具链版本、统一 MSRV、把壳层纳入 lint 脚本。

#### P2-10 JSON-RPC 错误应答 id 违规

- 【位置】`engine/ai-engine/src/server.rs:216-219, 234-240`、`engine/ai-protocol/src/types.rs:67-74`
- 【现象】解析失败/信封不合法统一回 `RequestId::Number(0)`；JSON-RPC 2.0 规范要求不可判定时 id 为 null，且与合法 `id:0` 撞车致错误误配到错误请求。
- 【根因】`RpcResponse.id` 无 Null 变体。【修复建议】id 改 `Option<RequestId>`，不可判定时回 null，可判定时回显原 id。

#### P2-11 出站无界队列无背压

- 【位置】`server.rs:193`（`mpsc::unbounded_channel`）、`pump.rs:62-78`、`batch.rs` 头注「批量与背压」
- 【现象】客户端阻塞时 pump 持续产 delta → 内存无界增长；所谓背压只有分批没有流控。
- 【修复建议】有界 channel 或 delta 丢弃/合并策略。

#### P2-12 P11 字段 TS 导出语义失真（bigint / 必填可空）

- 【位置】`engine/ai-protocol/src/types.rs:2834+`（ReportGenerateParams/Result）、`ui/src/protocol/generated/ReportGenerateParams.ts:20-45`、`ReportGenerateResult.ts:15`
- 【现象】`Option<u64> offset/limit`、`u64 totalChars` 生成 `bigint`（JSON 实为 number）；`title: string | null` 为「必填可空」，而 serde `skip_serializing_if` 会省略键——与 `EngineCapabilities.reports?: boolean` 用的 `ts(optional)` 约定不一致（rev 等既有同病，新字段延续历史债）。
- 【修复建议】`ts(type="number")` + `ts(optional)`；趁 P11 一并定下导出约定（教训已有：Option 字段必须带 ts(optional)）。

#### P2-13 report-spec 吞 Windows 盘符路径

- 【位置】`cli/aic/src/script.rs:410-416`
- 【现象】裸 token 判定 `!tok.contains(':')` → `output-to:C:\rep.txt` 归入 ignored，报告静默落 stdout。
- 【修复建议】识别盘符前缀 `[A-Za-z]:`。

#### P2-14 sync_empty_dirs=false 时空目录判定恒查左树

- 【位置】`engine/ai-dir/src/sync.rs:413-417`
- 【现象】实测恒查 `roots.0+rel`：右侧孤目录路径不在左树 → list_dir 失败 → 判非空，右孤空目录的复制/删除照常执行。
- 【修复建议】按行所在侧探测（或双侧取或）。

#### P2-15 filter contains/not_contains 整文件载入内存

- 【位置】`engine/ai-dir/src/filter.rs:451-454`
- 【现象】先 `read_file_bytes` 整文件再截 1MB，大文件整载内存。
- 【修复建议】改 read_range(0, CONTENT_SCAN_LIMIT)。

#### P2-16 Dir 排除规则误排除同名文件

- 【位置】`engine/ai-dir/src/filter.rs:289-294`
- 【现象】`file_allowed` 步骤 1 把文件自身全路径也比对 Dir 排除规则：与被排除目录同名的**文件**被误排除（BC 目录掩码仅命中目录）。
- 【修复建议】深度取 `1..segments.len()`。

#### P2-17 UI/引擎双份过滤器语义漂移

- 【位置】`ui/src/.../dirFilters.ts:21-197` vs `engine/ai-dir/src/filter.rs`
- 【现象】UI 侧重复实现掩码引擎且为子集：缺 `*.*` 历史特例、尾句点 no_dot、`.../` 任意深度、includeDirs 子树包含（fileAllowedBySpec 完全忽略）——本地预览 ≠ 引擎判定。
- 【修复建议】以 NameFilterSpec 送引擎求值或抽公共语义测试对拍，收敛到单一语义源。

#### P2-18 flat_rows 平铺口径污染统计，切回树形基数漂移

- 【位置】`engine/ai-dir/src/session.rs:1045-1070, 1145-1155`
- 【现象】flat_rows 用平铺口径整体覆盖 st.stats，set_display 切回树形只重发行不重算统计 → 基数漂移且后续增减继续叠加在错值上。
- 【修复建议】退出 flat 时按 arena 重算 stats。

#### P2-19 zip 中心目录条目名不清洗 + VfsPath::join 不规范化 `..`

- 【位置】`engine/ai-vfs/src/zip.rs:114-121`、`lib.rs:244-252`
- 【现象】条目名 `../`、`/abs`、盘符不拒绝；`join` 亦不规范化 `..` 段；list_dir 会把 `..` 段暴露为目录名。当前 zip 只读 + execute sanitize_rel 暂兜住写面，但读/展示与未来解压面越界风险真实（zip-slip 纵深缺口）。
- 【修复建议】解析时拒绝/剔除非法名，join 做词法规范化并拒 `..`。

#### P2-20 retryEngine 只复位不重连

- 【位置】`ui/src/ipc/engineProcess.ts:267-275`
- 【现象】置 connecting 后无 `getEngineClient()` 调用，需等下一次业务请求才连；手动重试后横幅可能长挂「正在连接」。

#### P2-21 rev 机制空转（失序/resync 检测未实现）

- 【位置】`ui/src/ipc/dirRowCache.ts:117-139`（完全忽略 `DirDelta.rev`）、`mockEngine.ts:283-414`（rev 硬编码 3/4/5 非单调，`as unknown as bigint` 掩盖类型失真）、`engineProcess.ts:222`（resync 固定 `rev: 0` → resync 必走 replay）

#### P2-22 parent-* 作用域写回抛错但 UI 暴露选项；四层合并未端到端

- 【位置】`ui/src/session/service.ts:400-403`（throw）、`SettingsDialog.tsx:118-127`（preview 仅喂三层）、`scopes.test.ts:17`
- 【现象】LAYER_ORDER 的 parentKind/parentPair 生产路径从未被填充，四层合并仅函数级测试覆盖。

#### P2-23 大文件编辑性能：每击键全量 rediff + 双窗全量回灌

- 【位置】`ui/src/views/text-compare/AlignedPane.tsx:169-183`、`TextCompareView.tsx:414-426`
- 【现象】setContent 每次全量替换整篇文档并重建全部装饰；每击键同步 rediff。万行级 diff 打字卡顿。
- 【修复建议】增量 changes + 装饰 map 映射、rediff 防抖。

#### P2-24 壳层 IPC 边界无路径约束

- 【位置】`ui/src-tauri/src/lib.rs:115-123`（engine_start 接受任意可执行路径）、`280-338`（settings_* 接受任意文件路径）
- 【现象】webview 被注入即可任意读写/起进程。
- 【修复建议】settings_* 校验路径落在 base_dir 内，engine path 限白名单。

#### P2-25 progressive `done()` 返回粗粒度过时统计

- 【位置】`engine/ai-core/src/progressive.rs:277, 297-299`
- 【现象】`renumber` 返回值被丢弃，`self.summary` 自 build_coarse 后未更新；精化改变 section 形态后 Done 数据错误。【修复】`self.summary = renumber(...)`。

#### P2-26 myers `coalesce_ops` 交换不彻底

- 【位置】`engine/ai-core/src/myers.rs:143-154`
- 【现象】`[Ins,Ins,Del]` 单趟后成 `[Ins,Del,Ins]`，破坏「先删后增」不变量（注释声称保证）；build_sections:194 有兜底，但块结构与全量重 diff 可能不一致。【修复】交换后回退一步继续扫描。

#### P2-27 keys 嵌入空白折叠误伤行首/行尾空白（三类开关不独立）

- 【位置】`engine/ai-core/src/keys.rs:91-103`
- 【现象】`embedded_ws_unimportant` 对 `[start..end)` 全段折游程为单空格，未剥离开头/结尾游程：`"  a"` 与 `" a"` 在 `leading_ws_unimportant=false` 时被判同键。【修复】仅折叠中段游程。

#### P2-28 比较键 `\u{0}` 后缀可构造碰撞

- 【位置】`engine/ai-core/src/lines.rs:170`、`keys.rs:66-68`
- 【现象】行文本含「`\u{0}crlf`」等字样时与「另一行+EOL 标签」同键，骨架层错判相等（对称但非单射）。【修复】对文本长度或转义做域分隔。

#### P2-29 report.rs `patch_sections` 折叠逻辑缺陷

- 【位置】`engine/ai-core/src/report.rs:928-968`
- 【现象】不重要/忽略 section 折 Equal 时 `right: Some(l)` 直接用左区间（右坐标错），且 Insert 类（left=None）被整体丢弃 → patch 行覆盖断裂、行号错位。【修复】保留真实右区间，Insert 折叠需同时保留右行。

### P3 — 建议

#### P3-1 pb_bench 编译开销

- 【位置】`tests/Cargo.toml:65-67`
- 【现象】`pb_bench.rs` 注册为 `[[test]]` 但两用例均 `#[ignore]`（不会被默认执行，「误收」不成立），仅剩编译开销。可移入 feature 门进一步降开销。

#### P3-2 F7（deny.toml × rsproxy 镜像冲突）证据不足，大概率不成立

- 【位置】`deny.toml:55-58` vs `.cargo/config.toml:3-8`
- 【现象】Cargo.lock 中 56 处 source 均为官方 crates.io index（0 处 rsproxy）——source replacement 对 resolve 源 ID 透明，allow-registry 与 lockfile 一致，「rsproxy 触发 unknown-registry」证据不足；cargo-deny 未安装无法实测。
- 【修复建议】安装 cargo-deny 实跑一次定论（与 P2-1 CI 一并解决）。

#### P3-3 golden bless 自证风险（部分存在）

- 【位置】`tests/src/lib.rs:209-224`（UPDATE_GOLDEN=1 写快照即通过）
- 【现象】无 CI 把关 bless。但 golden_test.rs、report_test.rs 另有大量独立语义断言（错误码、patch roundtrip、确定性、stats 不泄内容），非纯快照自证；patch/algodiff 对拍有双向断言与编辑量守恒防「归类掩盖真 bug」。风险可控，建议 bless 后强制 diff 审查记录。

#### P3-4 substitute_vars 字节/字符下标混用

- 【位置】`cli/aic/src/script.rs:207/218`
- 【现象】`i += name.len()+2`（字节）作用于 `Vec<char>` 下标，非 ASCII 变量名（`%名%`）替换后文本截断/错位（无 panic）。

#### P3-5 超长行无上限 / jsonrpc 版本不校验

- 【位置】`server.rs:198`（`lines()` 无行长限制，单行可 OOM）；`types.rs:48-53`（`"jsonrpc":"1.0"` 照常受理）

#### P3-6 参数静默忽略族 + cancel 能力超卖

- 【位置】`script.rs:1308`（`_create`）、`parse_report_spec`（display/patch 非法值→None 而 layout 却报 106）、`report_cmd.rs:266`（未知列忽略）、`exec_attrib`（`_paths` 仅改左侧）
- 【现象】静默忽略偏多；且 **cancel 能力超卖**：仅 dir_open/dir_execute 接 CancelFlag，report/generate 大渲染不可取消，能力位却报 `cancel:true`（`handlers.rs:115`）。

#### P3-7 aiscript 退出码语义小不一致

- 【位置】`script.rs:2680-2695`
- 【现象】0/1/2/11-14 主契约一致且有锚定测试（`lib.rs:423-439`）；但 aiscript 的 105/106/107「用法错误→106、--var 错误→2」不一致，105 语义与 docs/03「加载错误」措辞有出入。

#### P3-8 SQLite 缓存只增不删 + crc 吞错

- 【位置】`engine/ai-dir/src/cache.rs:138-153, 129`
- 【现象】仅 upsert 不删同路径旧版本行（测试还断言旧行可查）→ 表无限增长；`u32::try_from(crc).unwrap_or(0)` 吞错可能回 0 致误判同。跨进程 WAL 无 busy_timeout、写失败静默（可接受取舍）；mtime 不变+内容变的陈旧命中为 ADR-004 文档化风险。

#### P3-9 execute 部分失败语义不一致

- 【位置】`engine/ai-dir/src/execute.rs:280-286, 348-353`
- 【现象】Move 失败不回滚已复制内容（部分状态）；Delete 目标不存在记 Failed 而非 Skipped，中断重放误报失败；rename 校验拒绝覆盖与 trait「覆盖语义」（lib.rs:373）矛盾。sync 无持久状态机/日志，重入依赖调用方重扫。

#### P3-10 快速测试短路语义与 FR 表述的出入（待证实）

- 【位置】`engine/ai-dir/src/quick.rs:89-103`、`session.rs:497-507`、`merge3way.rs`
- 【现象】Auto 短路（快速项全同不复核内容）符合 FR-DIR-011「快速相同则跳过」，秒容差/DST 吸收的 mtime 差也直接判 Same（文档化取舍）；Always 下属性/大小写差异不被内容覆盖与 FR-DIR-011 表述有出入（待证实）。session.rs:497-507 配对文件仅左侧过 OtherFilter（待证实）。merge3way classify_file 注释「最多两次」实为三次、未接 HashCache。

#### P3-11 协议边界无运行时校验

- 【位置】`ui/src/ipc/engineProcess.ts:257-259`（`params as DirDelta` 等强制转型）；bigint/number 双语义依赖注释自律。

#### P3-12 Space 标记双 window-capture 监听 hack

- 【位置】`ui/src/views/folder-compare/FolderCompareView.tsx:538-552`（注释自认依赖注册序），stub 命令与视图逻辑并跑，脆弱。

#### P3-13 UI 测试盲区

- 【位置】ipc/engineProcess、ipc/tauriTransport、shell/shortcutEngine、shell/commands、shell/sliceSessionUi、views/text-compare/{cmDecorations,textModel,textSession}、session/storageAdapter 均无测试；P1-9 重连竞态正落在无测区。

#### P3-14 TabBar 无方向键导航 / roving tabindex

- 【位置】`ui/src/shell/TabBar.tsx:54-76`；键盘用户只能 Tab 逐个过——对比工具核心用户群受影响。

#### P3-15 incremental Append 路径不发 Done 事件（与 Replace 路径契约不一致）

- 【位置】`engine/ai-core/src/incremental.rs:111-116`

#### P3-16 inline 超时降级行被静默丢弃

- 【位置】`engine/ai-core/src/inline.rs:267`
- 【现象】truncated 行不进结果，调用方不知该行对存在；建议保留条目并标 truncated。

#### P3-17 options.rs 锚点越界错误信息笔误

- 【位置】`engine/ai-core/src/options.rs:203`（`right {a:?}` 应为 right 变量笔误）

#### P3-18 hex 块等价仅凭 FNV-1a 哈希、未回验字节（待证实）

- 【位置】`engine/ai-core/src/hex.rs:225-240`
- 【现象】哈希相等即判块相同，碰撞即误报相同（未核完 emit 路径，待证实）。

#### P3-19 incremental 锚点边界纯插入可能错过重对齐（设计取舍，待证实）

- 【位置】`engine/ai-core/src/incremental.rs:169`（同 mine 坐标双边界时窗口取 other 较大者）

---

## 4. 测试与工具链缺口

### 4.1 本次实测数据

- 引擎 workspace（`--no-fail-fast`）：通过 582+，失败 7——全部与未提交 P11 改动相关（4 个 patch 回归 + 3 个 golden/replay 快照）；提交入库前 P0-1 必须修复。
- UI：383/383 通过（34 文件）。
- clippy：1 错误（P11 在制品死代码）。

### 4.2 覆盖盲区

| 区域 | 现状 |
| --- | --- |
| e2e 协议面 | 仅 6 方法 happy path，无错误/取消/大 payload |
| merge3 对拍 | 工具缺失时静默跳过（P2-6） |
| fixtures 一致性 | 生成器与 Rust 期望无自动比对（P2-7） |
| Tauri 壳层 | 不在根 workspace，无 lint/test 覆盖（P2-9） |
| 壳层/引擎进程边界 | 无崩溃重启/僵尸进程自动化测试 |

---

## 5. 风险矩阵

| 风险 | 概率 | 影响 | 可探测性 | 等级 |
| --- | --- | --- | --- | --- |
| P11 悬空改动丢失（崩溃/误操作） | 中 | 高（~1500 行+304K 快照） | 高（git status 可见） | **高** |
| patch 幂等回归进入主干 | 高（已在工作树） | 高（补丁重复应用出错） | 高（4 测试红） | **高** |
| 协议边界 panic → 客户端挂死 | 低 | 高（UI/CLI 无限等待） | 低（无应答超时） | **高** |
| 崩溃自动重启链路失效 | 中（崩溃即触发） | 高（需手动重启） | 中 | **高** |
| flat 模式文件操作错路径 | 中 | 高（数据安全） | 中 | **高** |
| expand/worker 死锁 → 目录永不扫描 | 中 | 高（结果静默缺失） | 低（进度虚报完成） | **高** |
| 写路径 symlink 重定向写穿 | 低 | 高（任意文件覆盖） | 低 | **高** |
| 原子写名实不符 → 断电丢会话 | 低 | 中 | 低 | **中高** |
| 非同名对齐读错路径 | 中 | 中（判定错误） | 中 | **中高** |
| 无 CI 门禁静默失效 | 高 | 中 | 低（无探测点） | **高** |
| PROGRESS.md 失真误导续跑 agent | 高（已发生） | 中 | 中 | **中高** |
| 出站无界队列 OOM（客户端阻塞） | 低 | 中 | 低 | 中 |
| 工具链漂移（stable 未钉版本） | 低 | 中 | 低 | 中 |
| 对拍空转掩盖算法回归 | 低 | 高 | 低 | 中 |
| golden bless 自证 | 低 | 中 | 中 | 低中 |

---

## 6. 附录

### 6.1 命令输出摘录

```
cargo test --workspace --no-fail-fast  → EXIT 101（7 failed）
  patch::tests::apply_already_applied_detected   (ai-core, patch.rs:1164)
  handlers::p9_tests::patch_apply_rpc_roundtrip  (ai-engine, handlers.rs:3094)
  parser_tolerance_goldens                       (patch_matrix, patch_test.rs:450)
  patch_layout_roundtrip_with_aic_diff_and_apply (report_matrix, report_test.rs:392)
  golden_merge3_session_snapshot / golden_text_paging_inline_and_importance_snapshot
  golden_dir_and_text_session_snapshot           (capabilities 新增 reports:true)
  record_then_replay_is_identical                (protocol_fixtures, 同上能力位)
cargo clippy --workspace --all-targets -- -D warnings → EXIT 101
  error: method `read` is never used (tests/script_matrix/script_test.rs:52)
cd ui && pnpm test → 34 files / 383 tests 全绿
```

### 6.2 已排除的误报 / 已核无问题（避免重复劳动）

- `tests/Cargo.toml` 用 `[dev-dependencies]` 放 ai-* 合法（仅 `[[test]]` 集成目标使用）。
- 根 workspace 不含 `ui/src-tauri` 是 ADR-001 有意解耦（覆盖缺口本身仍是问题，见 P2-9）。
- `pb_bench` 不会被 `cargo test` 默认执行（`#[ignore]`）。
- F7（deny.toml × rsproxy）证据不足，暂不记问题（P3-2）。
- 协议/CLI 已核无问题：stdio 无 stdout 污染（日志全走 stderr）；batch/pump 串行链 + rev 单调/幂等、取消注册表生命周期测试齐；report HTML 有 html_escape；向后兼容锚定测试（能力位缺省 true、最小载荷、枚举字节锚定）充分；report_generate 分块按 char 切片不劈 UTF-8；退出码 0/1/2/11-14 有锚定测试。
- UI 已核无问题：架构分层（纯函数数据面/IPC 适配/壳=管道）与注释纪律为同类高水位；session 恢复、快捷键冲突检测、delta 缓存等纯函数层测试扎实。
- ai-dir/ai-vfs 已核无问题：对齐契约、diff3 分类表、VFS 能力位降级、删除路径护栏扎实；CRC 与 zip 预计算一致性（crc32.rs）核对无误。

### 6.3 后续建议排期

1. **立即（P11 合库前，阻断项）**：修 P0-2 patch 回归（`has_delete`→「无 Add 行」）→ 修 P1-10 report.rs 下标错配 → 重固化 golden/replay（核对仅 `reports:true` 一行）→ 修 P2-5 clippy → 验收提交 P11 入库（解除 P2-2/P1-2b 丢失风险）。
2. **本周（高风险清偿）**：P0-1 崩溃重启探活、P1-1 panic 边界 catch_unwind、P1-8 flat id 语义、P1-2/P1-3/P1-4（ai-dir 三缺陷）、P1-6/P1-7（UI 原子写/关窗丢数据）；修 PROGRESS.md 状态源（P2-3）；补最小 CI（P2-1）。
3. **下阶段（质量与体验）**：P1-5/P1-9/P1-11、对拍防跳过（P2-6）、UI/引擎过滤器收敛（P2-17）、fixtures 一致性（P2-7）、e2e 补错误/取消路径（P2-8）、工具链钉版（P2-9）、TS 导出约定（P2-12）、大文件编辑性能（P2-23）、壳层 IPC 路径约束（P2-24）。
4. **持续**：P3 各项随触碰文件顺手修；「待证实」4 项（P3-10/18/19、hex 哈希回验）专项核实。
