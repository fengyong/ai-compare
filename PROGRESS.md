# ai-compare 自主开发进度（状态源文件）

> 本文件是自主开发的**唯一状态源**：任何 agent（含定时看护任务）接手工作时，先读此文件判断进度与断点，推进后必须更新此文件。

| 项目 | 内容 |
| --- | --- |
| 项目目标 | 完整复刻 Beyond Compare（BC5）功能，产品名 ai-compare |
| 工作目录 | /Users/ola/drax-toolbox/ai-compare |
| 预算说明 | 2026-11-13 23:00 后 token 预算非常充足（5 亿），可加大并行度；23:00 前适度并行 |
| 授权 | 用户已授权独立自主开发，无需逐项确认 |

## 里程碑总览

- [x] M0.1 需求调研：`docs/01-需求规格说明书.md`（1548 行，163 个编号需求，P0/P1/P2）
- [x] M0.2 开源竞品与技术调研：`docs/02-开源竞品与技术调研报告.md`（571 行；结论：Rust headless 引擎 + Tauri 2 UI 解耦）
- [x] M0.3 架构设计 + 开发计划：`docs/03-架构设计文档.md`（1033 行，10 条 ADR）、`docs/04-开发计划.md`（584 行，P0~P17 阶段）✅ 已验收
- [x] **M1 Walking Skeleton ✅ 完成**（对应 docs/04 P0 阶段，提交 1297af9/145c61e/a0801ca）：
  - [x] 引擎线：Rust workspace 五 crate + ai-engine stdio + aic CLI（119 测试、clippy 零警、Myers 300 轮 oracle 对拍、`aic diff`≡`diff -u`、38 个 ts-rs 类型）
  - [x] UI 线：Tauri2+React 壳 + 目录/文本视图 + 协议客户端（44 测试、浏览器端到端冒烟）
  - [x] 集成验收：UI 切 generated 类型、引擎路径修正（workspace 根 target）、`pnpm e2e:engine` 真实引擎全链路全绿（initialize→dir/open→delta 断言→text/sections→shutdown）；顺带修复 dirRowCache 删除未清 id 索引 bug；51 UI 测试 / build / lint / cargo check 全绿
- [x] **M2 MVP ✅ 完成（02:1x 联合验收 + 03:5x P5 交付，P0 需求全量覆盖）**：五线交付（P1/P2/P4/P3/P5）
  - [x] P1 diff 核心（91 测试、四算法、重要性双键、增量重 diff、对拍矩阵）
  - [x] P2 目录引擎（惰性扫描、BC 过滤器全集、zip CRC、SQLite 缓存、同步状态机、10 万项 PB-03 1.54s 达标）
  - [x] P4 UI 壳会话（主页/多窗口/持久化/快捷键/重连）
  - [x] P3 CLI 全集+文本 RPC+P1 协议映射（318 引擎测试、git difftool 冒烟、协议向后兼容逐字节锚定）
  - [x] P5 文本视图完备化（214 UI 测试、可编辑/折叠/高亮/查找/编码/概览条）——**MVP 用户故事全量闭环**
- [x] **M3 前半（P3/P5）✅ 验收完成（03:5x）**
- [ ] **M3 V1.0（进行中）**：并行启动 P6（文件夹比较视图+文件操作）+ P7（三向文本合并引擎+视图）
- [ ] M4 V2.0（含 Pro 级功能）

## 当前阶段与断点

**当前**：M1 Walking Skeleton（docs/04 P0 阶段）双线并行开发中——引擎线 agent、UI 线 agent 均在后台运行。

**下一步（任一 agent 完成后）**：
1. 验收该线交付物（引擎线：`cargo build/test/clippy` 全绿、协议收发演示、aic 退出码；UI 线：`pnpm build/test` 全绿、cargo check）
2. 两线都完成后做集成验收：引擎+UI 对接联调、`aic diff` 与 `diff -u` 对拍、PB-04 冒烟基准 → M1 打勾 + git commit
3. M1 完成后进入 M2（docs/04 P1 diff 核心完备化 + P2 目录引擎完备化可并行），23:00 后加大并行度
4. 断点恢复入口：若两 agent 均中断且无登记，按本节从第 1 步重新验收重启

## 运行中任务登记

| 时间 | 任务 | 状态 |
| --- | --- | --- |
| 2026-11-13 21:1x | Agent「架构设计与开发计划」→ docs/03 + docs/04 | ✅ 完成 |
| 2026-11-13 21:3x | Agent「P0 引擎线」→ Rust workspace + 引擎 crates + aic CLI + 测试 | ✅ 完成并验收：119/119 测试、clippy 零警、退出码矩阵 0/1/2、`aic diff` 与 `diff -u` hunks 等价（主会话复验 + 提交）；Myers 经 300 轮 DP-oracle 对拍；PB-04 冒烟 1 万行 0.8ms |
| 2026-11-13 22:5x | Agent「M1 集成」→ UI 切真实引擎 + generated 类型合入 + e2e:engine 脚本 | ⚠️ 23:04 因 provider 5h 配额中断于半程；主会话接手完成全部剩余工作并验收（a0801ca） |
| 2026-11-13 23:59 | 看护定时任务重建（旧任务因模型切换失效，新任务同节奏每 20 分钟） | active |
| 2026-11-13 23:36 | Agent「P1 diff 核心完备化」（engine/ai-core，算法矩阵/重要性/增量重diff） | ✅ 完成并验收（01:1x）：ai-core 91/91 测试、clippy 零警、对拍 19/21 与 difflib+git 逐 hunk 一致（2 歧义用例性质校验+编辑量守恒）、PB-05 20 万行 standard 24.9ms；提交（仅 P1 文件）；遗留——协议 DTO 映射（Replace/ImportanceUpdate/双键/手动对齐）待 P2/P5 排期 |
| 2026-11-13 23:36 | Agent「P2 目录引擎完备化」（ai-dir/ai-vfs/protocol/cli，惰性扫描/过滤器/缓存/同步状态机） | ✅ 完成并验收（02:1x）：workspace **259 测试**全绿、clippy 零警、golden 重固化仅 3 能力字段差异、新二进制 e2e 通过、PB-03 10 万项 1.54s 达标、PB-10 delta 23.3ms；修复 5 个关键缺陷（含单线程 runtime 死锁红线）；**M2 联合验收通过并提交** |
| 2026-11-14 02:2x | Agent「M3/P3 CLI 全集+文本 RPC 接线+P1 协议映射」（引擎侧：cli/tests/protocol/ai-engine/ai-core API） | ✅ 完成并验收（03:1x）：workspace **318 测试**全绿（+59）、clippy 零警、新二进制 e2e 含 inline=full 路径、git difftool 冒烟 10 断言通过、协议向后兼容有逐字节锚定测试；提交 8b67a72（仅 P3 scope+generated/） |
| 2026-11-14 02:2x | Agent「M3/P5 文本比较视图完备化」（ui/ 独占：编辑/折叠/语法高亮/查找替换/编码/概览条） | ✅ 完成并验收（03:5x）：214/214 测试（+80）、build/lint/e2e 全绿、冒烟 8 场景实录（编辑→重 diff→撤销→折叠→查找替换 481 命中→概览条跳转→会话缓存存活）；冒烟期自修 3 真实缺陷；提交 5fb8090 |
| 2026-11-14 04:0x | Agent「M3/P6 文件夹视图与文件操作」（ui/ 独占 + ai-vfs 写 API + ai-dir 执行层） | ✅ 完成并验收（04:1x）：ai-vfs 21 + ai-dir 79 测试、UI 265（+51）、e2e 过、冒烟 8 场景含镜像落盘 53 项；**主会话全量验证时遇 P7 中间态瞬态失败（2 失败/5 警告），隔离按 crate 复验 P6 scope 全绿后精确提交**；交接——`dir/execute` RPC 提案（DTO 已同形实现于 UI+mock）待下波引擎接线；已代 P7 重固化 golden（threeWayMerge 能力位） |
| 2026-11-14 04:0x | Agent「M3/P7 三向合并引擎」（ai-core merge3 + ai-protocol 本波协议权 + ai-engine + aic merge + diff3/git 对拍） | ✅ 完成并波次联合验收通过（04:3x）：workspace **420/0 全绿**、clippy 零警、e2e 过；**git merge-file 对拍 28/28 逐字节一致（100%）**，GNU diff3 18/28（差异归类：GNU -A bracketed 语义 vs git 语义，以 git 一致为准）；重要性开关冲突缩小行为有测试；提交入库 |
| 2026-11-14 04:3x | Agent「M3/P8 同步与文件夹合并」（ai-dir 同步执行+文件夹合并数据层 + ui/ 独占视图） | 运行中 |
| 2026-11-14 04:3x | Agent「M3/P9 表格/十六进制引擎」（ai-core 表格列模型+hex 字节视图模型 + ai-protocol 本波协议权 + ai-engine + cli） | 运行中 |
| 2026-11-13 23:36 | Agent「P4 UI 壳与会话体系」（ui/，主页/多窗口/会话持久化/ViewRegistry） | ✅ 完成并验收（含一次并发中断+SendMessage 原地续跑）：134/134 测试（+83）、build/lint/e2e:engine 全绿、浏览器冒烟全流程通过（00:47 主会话复验）；遗留项见 ui/README P4 边界 |

### M3 断点（下一步，P3/P5 并行）

1. **P3 CLI 全集 + 文本 RPC 接线**（agent，权限：`cli/`、`tests/`、`engine/ai-protocol/`、`engine/ai-engine/`、`engine/ai-core/` 对外暴露）：docs/04 §P3（aic/aicc 双形态、退出码 0/1/2/11/12/13/14、`--record` 已有补 fixture、git difftool 冒烟）+ P1 协议 DTO 映射（SectionEvent::Replace/ImportanceUpdate、双键、手动对齐选项上协议面）+ text/lines RPC 引擎入口开放
2. **P5 文本比较视图完备化**（agent，权限：`ui/` 独占）：docs/04 §P5（FR-TXT-001~004/013/014/020~023/030、FIND-001、UNDO-001、ENC-001、EDIT-001——折叠/语法高亮/查找替换/编码切换/概览条/编辑能力，消费 P3 新协议面；若 P3 未就绪先做 UI 侧不依赖协议扩展的部分）
3. 文件权限互斥（P3=引擎侧全权、P5=ui/ 独占）；协议类型修改权随 P2 完成移交 P3
4. 完成后：M3 阶段验收 + 启动 P6（文件夹比较视图与文件操作，12 人周最大件）前评估是否三线并行
| 2026-11-13 21:3x | Agent「P0 UI 线」→ Tauri2+React 壳 + 目录/文本视图 | ✅ 完成并验收：44/44 测试、build 零错误、cargo check 过、浏览器端到端冒烟通过（22:31 主会话复验 + 提交 1297af9） |
| 2026-11-13 21:44 | 后台：rustup default stable 修复工具链 | ❌ 失败：与原安装任务并发竞争 downloads 目录（教训：勿并行跑两个 rustup） |
| 2026-11-13 21:52 | 后台：原 rustup 安装任务仍在下载 stable 工具链（40MB/~200MB） | ⛔ 已主动终止：官方源仅 1.3MB/min，预估还需 1.5h+ |
| 2026-11-13 21:55 | 重装：rsproxy.cn 镜像装 stable 工具链 | ✅ 完成：rustc/cargo 1.100.0，40 秒装完；另建项目级 `.cargo/config.toml`（crates.io 走 rsproxy 镜像） |

（agent 完成或中断时更新此表；看护任务据此判断是否有工作进行中）

## 开发约定

- 每完成一个模块/阶段：更新本文件 + git commit（信息格式：`M<里程碑>-<模块>: 摘要`）
- **开发 agent 不自行 git commit、不编辑 PROGRESS.md**（由主会话统一提交与登记，避免并发冲突）
- 代码须带单测；引擎层（Rust）保持无 UI 依赖，可全量单测
- 遵循 docs/03 架构决策；如需偏离，先在 docs/03 追加 ADR 再动手
- 历史决策详见 docs/01、docs/02
- 工具链：node v26.8.1 / pnpm 12.18.0 / rustc+cargo 1.100.0（stable，已装好）；crates.io 已配项目级 rsproxy.cn 镜像（`.cargo/config.toml`）；cargo 在 `$HOME/.cargo/bin`，agent 执行前先 `source "$HOME/.cargo/env"`

## 事件日志

- 2026-11-13 21:0x：目录确认为空，启动双调研 agent
- 2026-11-13 21:1x：调研完成（docs/01、docs/02 验收通过）；启动架构设计 agent；建立本进度文件与看护定时任务（每 20 分钟）
- 2026-11-13 21:2x：git init（main 分支）+ 首次提交（docs/01、docs/02、PROGRESS.md）；等待架构 agent 完成
- 2026-11-13 21:3x：M0.3 完成（docs/03 1033 行 / docs/04 584 行，验收通过并提交）；rustup 后台安装；启动 P0 引擎线 + UI 线双 agent
- 2026-11-13 21:44：**首次看护巡检**：两开发 agent 均存活（运行中，早期读文档阶段）；发现 rustup 默认工具链缺失 → 已后台修复；docs/01~04 齐全；无中断，无需重启
- 2026-11-13 21:55：**工具链危机解除**：官方源下载过慢（1.3MB/min）→ 终止原任务改用 rsproxy.cn 镜像，40 秒装完 rustc/cargo 1.100.0；建项目级 crates 镜像配置；UI 线已有产出（ui/ 目录脚手架成型），引擎线 agent 曾停滞 15min（疑等待 cargo），现应自行恢复；提交 .cargo/config.toml
- 2026-11-13 22:04：**第 2 次看护巡检**：双线健康——引擎线已恢复活跃（6 秒前有进展，产出 engine/ai-protocol+ai-core 含 myers/inline/sections/options、根 Cargo.toml/deny.toml/rust-toolchain.toml），UI 线活跃（ui/src 28 个文件）；rust-toolchain.toml 仅钉 stable（无额外下载）；无中断，不干预；产出待 agent 自验后由主会话统一验收提交
- 2026-11-13 22:24：**第 3 次看护巡检**：双线活跃（引擎 13s 前 / UI 19s 前有进展）；引擎 .rs 增至 25 个（ai-dir/ai-engine 应已开写），UI node_modules 已装、dev server 已在 5173 端口可访问；尚无 target/ 构建产物（引擎线处于写码→编译转换段）；无中断，不干预
- 2026-11-13 22:31：**UI 线完成并验收提交**（1297af9）：主会话复跑 pnpm test（44/44）+ build（零错误）核实；亮点——mock 引擎降级、delta 订阅挂缓存层修复回传丢失 bug、wordDiff LCS+gap 分组最小化高亮；遗留——待引擎二进制联调、PB-04 帧率基准属后续阶段。引擎线继续运行中
- 2026-11-13 22:44：**第 4 次看护巡检**：引擎线活跃（10s 前有进展），engine/cli/tests 共 113 文件，target/debug 已有 21 项构建产物（编译/测试循环中）；无中断，不干预
- 2026-11-13 22:5x：**引擎线完成并验收提交**（145c61e，83 文件）：主会话复验 119/119 测试、clippy 零警、退出码 0/1/2、`aic diff`≡`diff -u`（hunks 逐字节一致）；Cargo.lock 改为提交；启动集成 agent（UI 切真实引擎 + e2e:engine）
- 2026-11-13 23:04：**第 5 次看护巡检**：集成 agent 活跃（5s 前，读文档/类型比对早期）；已过 23:00 进入预算充裕期，集成完成后按计划并行 P1/P2/P4；无中断，不干预
- 2026-11-13 23:2x：**集成 agent 因 provider 5h 配额中断**（02:10:56 重置）；主会话接手集成收尾：修 4 个 TS 构建错误 + 3 个测试失败（重写 mockEngine 测试对齐 P0 协议面；**修复 dirRowCache removals 未清 indexOfId 的真 bug**）、引擎路径修正、编写 `ui/scripts/e2e-engine.mjs` 真实引擎端到端脚本并全绿、README 更新；51 UI 测试/build/lint/cargo check 全绿
- 2026-11-13 23:3x：**M1 walking skeleton 正式闭环**（a0801ca）；PROGRESS 登记 M2 断点：P1/P2/P4 三 agent 并行（文件权限互斥；协议类型只归 P2 改）；配额重置前若启动失败则由看护任务 02:10 后重试
- 2026-11-14 00:20：**新看护任务首巡**：P1/P2 存活运行中；P4 于 00:18 因并发数上限中断（冒烟阶段，磁盘 +1307 行已产出）；并发槽满故本轮不重启，下轮（或 P1/P2 完成通知到达时）SendMessage 续跑 P4；P2 已合法再生协议 TS 类型，P4 恢复后需对齐验证
- 2026-11-14 00:41：**第 2 次巡检**：P1/P2 存活（P1 长构建循环中、P2 活跃）；P4 已 SendMessage 原地续跑成功（上下文保留，从冒烟断点继续，任务书含协议类型对齐自检）；三线均运行中
- 2026-11-14 00:47：**P4 完成并验收提交**（7d5df90）：主会话复验 134/134 测试（+83）、build/lint/e2e 全绿；亮点——四层作用域合并、.bak 回滚演练测试、多窗口拖出合并、断线重连状态机；P4 曾并发中断→SendMessage 原地续跑自愈（看护机制首次完整闭环）
- 2026-11-14 01:00：**第 3 次巡检**：P1/P2 存活（P1 活跃、P2 跑重构脚本调整 ai-dir session.rs 统计口径——同步状态机数据层推进中）；不干预
- 2026-11-14 01:1x：**P1 完成并验收提交**：ai-core 39→91 测试全绿、clippy 零警；算法矩阵（Patience 手写 LIS+锚点回退 Myers、Histogram 经 imara-diff 直通 Token 序列）、重要性双键聚合、手动对齐/隔离/偏斜容差、编辑增量重 diff（窗口锚定+边界吸收）、真超时降级+DMP 清理；对拍生成器带 difflib×git 互校防呆；workspace 全绿待 P2 接线后统一复验（P1 报告的协调事项 #1）；已按文件权限只提交 P1 scope，P2 中间态保持未提交
- 2026-11-14 01:20：**第 4 次巡检**：P2 自验阶段（跑 workspace 全量测试，31 个 scope 文件未提交）；P1/P4 已完成；不干预
- 2026-11-14 01:40：**第 5 次巡检**：P2 活跃（自验中发现问题回改 ai-protocol 类型中）；不干预
- 2026-11-14 02:00：**第 6 次巡检**：P2 活跃（打磨 tests 语料生成器，收尾段）；不干预
- 2026-11-14 02:1x：**P2 完成并验收 + M2 MVP 正式收官**：workspace 259 测试全绿、clippy 零警、golden 重固化、新二进制 e2e 通过、PB-03 1.54s 达标；提交入库；M3 断点登记（P3/P5 双线）
- 2026-11-14 02:2x：**M3 双线启动**：P3（引擎侧 CLI 全集+文本 RPC+P1 协议映射）、P5（ui/ 独占文本视图完备化）
- 2026-11-14 02:40：**M3 双线首巡**：P3/P5 均活跃（读文档规划阶段）；不干预
- 2026-11-14 03:00：**第 2 次巡检**：P3/P5 高度活跃（模型调用 2s/5s 前各一次，尚在文档消化与设计段）；不干预
- 2026-11-14 03:1x：**P3 完成并验收提交**（8b67a72，41 文件 +3547）：aicc 等待型包装（difftool/mergetool 双约定）、退出码 11-13 细分、rules 重要性判等、diff 算法开关、text/lines+inlineDiff RPC、P1 协议映射（SectionEvent 双键/手动对齐，旧载荷逐字节不变有测试锚定）；git difftool 真仓库冒烟 10 断言通过；318 测试全绿；协调事项——UI 本地 TextLines 类型与 generated/ 重复，P5 交付时切 re-export；P5 继续运行
- 2026-11-14 03:20：**第 3 次巡检**：P5 活跃（构建验证/设计段，近 12 分钟无落盘属正常）；不干预
- 2026-11-14 03:5x：**P5 完成并验收提交**（5fb8090）：UI 214/214 测试（+80）、e2e 含 P3 新 RPC 面；**M2 MVP 用户故事全量闭环**（五线 P1/P2/P3/P4/P5 齐备）；累计引擎 318 + UI 214 = 532 测试
- 2026-11-14 04:0x：**M3 下一波双线启动**：P6（文件夹视图+文件操作：ui/ 独占 + ai-vfs 写 API + ai-dir 执行层）、P7（三向合并引擎：ai-core merge3 + 协议权移交 P7 + aic merge + GNU diff3/git merge-file 对拍）；文件权限零交集；P6 所需新 RPC 若涉及 ai-engine 写报告由主会话协调
- 2026-11-14 03:40：**P6/P7 首巡**：双线均活跃（文档消化段）；不干预
- 2026-11-14 04:00：**第 2 次巡检**：P6 极活跃（逐 crate 测试巡检循环中，已进入验证迭代）；P7 活跃（长构建段）；不干预
- 2026-11-14 04:1x：**P6 完成并验收提交**：VFS 写 API（6 方法默认 Unsupported 降级 + symlink 逃逸双重护栏）、执行层（dryRun/部分成功/取消/进度）、文件夹视图（多选/右键 8 操作/同步 5 预设计划预览/树平铺/过滤+抑制青绿还原）、mock 全落盘链路可演示；**验收插曲**：全量 workspace 复验遇 P7 中间态瞬态失败 → 按 crate 隔离复验 P6 scope 全绿后精确提交（并行验收新纪律：**交付线 scope 单独验证，全量绿留给波次联合验收**）；协调积压——`dir/execute` RPC 接线、批量重命名对话框（下波）
- 2026-11-14 04:3x：**P7 完成并波次联合验收提交**：workspace 420/0、clippy 零警、e2e 过；git merge-file 对拍 28/28 逐字节一致（100%）；GNU diff3 18/28 差异归类（以 git 语义为准）；累计引擎 420 + UI 265 = 685 测试；P8/P9 双线启动
- 2026-11-14 04:20+：**P8/P9 首巡**：双线均活跃（文档消化段）；不干预
