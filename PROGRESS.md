# ai-compare 自主开发进度（状态源文件）

> 本文件是自主开发的**唯一状态源**：任何 agent（含定时看护任务）接手工作时，先读此文件判断进度与断点，推进后必须更新此文件。

| 项目 | 内容 |
| --- | --- |
| 项目目标 | 完整复刻 Beyond Compare（BC5）功能，产品名 ai-compare |

> ## 🎯 产品原则（用户 2026-11-14 明确指令）
> **每个方面都尽可能和 Beyond Compare 一样**——交互、视觉语义、快捷键、退出码、布局、目录组织全面对齐 BC；个性化设计一律后置。
> 忠实度验收基准 = docs/01-需求规格说明书.md（BC 5.2.0 官方帮助逆向的 163 个 FR）+ 本机 BC 本体（/Applications/Beyond Compare.app 可实测对照）。
> 实现与 BC 语义不符 → 以 BC 为准修实现。


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
- [x] **M3 后半（P6/P6b/P7/P8/P9/P10/P11）✅ 全部验收完成（12:3x V1.0 波次联合验收）**：文件夹操作+遗留批/三向合并/同步/文件夹合并/表格hexpatch/文件格式规则/报告引擎+脚本处理器
- [x] **V1.0 波次联合验收 ✅（12:3x）**：引擎 791/0 + UI 383 + clippy 零警 + e2e + build 全绿；REVIEW-p11 深度评审清偿（P0-2/P1-10/P2 系由 P11 清偿，**P0-1 崩溃重启由主会话修复**：engine_start 探活清僵尸条目）；评审文档归档 docs/archive/REVIEW-p11.md；**P0 需求全量闭环**
- [ ] **M4 V2.0（进行中）**：P12（VFS 扩展压缩包/FTP）/P13（主题 i18n Shell 集成）/P14（图片/媒体/版本/注册表视图——Pro 级）依次推进；协调积压随行（设置→text/open rules 运行时接线、SNAPSHOT 全量格式、批量重命名正则模式）

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
| 2026-11-14 04:3x | Agent「M3/P9 表格/十六进制引擎」（ai-core 表格列模型+hex 字节视图模型 + ai-protocol 本波协议权 + ai-engine + cli） | ✅ 完成并波次联合验收通过（05:4x）：workspace **603/0 全绿**（+183）、clippy 零警、e2e 过；对拍 csv 18/18、xxd 6/6、git apply 11/11 结论一致（5 差异全归类）；**dir/execute 接线完成（P6 积压清偿）**；提交入库 |
| 2026-11-14 05:5x | Agent「M3/P10 文件格式与规则体系」（引擎 formats/rules 注册表+检测+持久化+协议+设置 UI；strings.ts 归 P10） | ✅ **全部完成并验收**（07:4x）：引擎侧（主会话接手收尾，699 全绿）+ UI 侧（SendMessage 续跑成功，383 测试 +22、build/lint/e2e 全绿、浏览器冒烟含持久化还原）；已提交；遗留——设置→text/open rules 运行时接线（入口在 text-compare 视图，权限外）移交下波协调 |
| 2026-11-14 07:5x | Agent「M3/P11 报告引擎与脚本处理器」（ai-core report 8 布局 + 脚本处理器 31 命令 + 协议权 + aic report/script） | ⚠️ 10:01 配额中断（12:08 重置）于「报告矩阵测试」前（脚本 21 测试已过）；▶️ 12:21 SendMessage 原地续跑（配额已重置，磁盘产出齐全：report.rs/aiscript bin/report_cmd/script） |
| 2026-11-14 09:00 | 看护定时任务第三次重建（模型切换；新任务并入 P11 权限边界与「中断点接近完成时主会话接手」规则） | active |
| 2026-11-14 09:21 | 新看护任务首轮巡检：P11 活跃（7s 前有进展，近 15 分钟无落盘——处于设计/构建段）；不干预 | - |
| 2026-11-14 09:41 | 第 2 轮巡检：P11 活跃（9s 前有进展）；不干预 | - |
| 2026-11-14 10:01 | 第 3 轮巡检：P11 活跃（24s 前有进展）；不干预 | - |
| 2026-11-14 05:5x | Agent「M3/P6b 文件夹视图遗留批」（ui/views/folder-compare 独占：批量重命名对话框/编辑菜单命令/空格标记；不动 strings.ts/store.ts/viewRegistry） | ✅ 完成并验收提交（07:1x）：UI 361 测试（+42）、build/lint/e2e 全绿（agent 冒烟已过+主会话复验门禁；中断于最后门禁，实测全绿）；批量重命名 DOS 掩码+正则/空格标记/编辑命令/快速比较/复制路径/基文件夹导航 |
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
- 2026-11-14 04:40：**第 2 次巡检**：双线高度活跃（P8 3s 前、P9 9s 前各有模型活动，进入实现段）；不干预
- 2026-11-14 05:00：**第 3 次巡检**：双线活跃（实现段推进中）；不干预
- 2026-11-14 04:3x：**P7 完成并波次联合验收提交**：workspace 420/0、clippy 零警、e2e 过；git merge-file 对拍 28/28 逐字节一致（100%）；GNU diff3 18/28 差异归类（以 git 语义为准）；累计引擎 420 + UI 265 = 685 测试；P8/P9 双线启动
- 2026-11-14 05:1x：**P8 完成并 scope 验收提交**：同步策略调度（SyncStrategy 四项可配 + run_sync 端到端 SyncReport）+ 文件夹三路合并数据层（align3/scan_merge/plan_merge，MergeRowState 自底向上聚合）+ 同步视图（预设→策略→预览→逐项改写→执行→报告导出）+ 三栏合并视图（决策继承展开/冲突进度/执行）；主会话修 3 处 trivial clippy 告警；**协议积压清单移交 P9/下波**（dir/operate 策略字段、dir/sync、dir/mergeScan|Plan|Execute、dirMerge 能力位、DirExecuteOp 切 re-export）
- 2026-11-14 05:4x：**P9 完成并波次联合验收提交**：表格引擎（RFC4180/列对齐两轮匹配/单元格 diff）+ hex 引擎（Full/Fast/None 三模式+分页视窗）+ patch 应用引擎（git 头容错/fuzz/CRLF）+ **dir/execute 接线（P6 积压清偿）**；workspace **603/0 全绿**（+183）；对拍 csv 18/18、xxd 6/6、git apply 11/11 结论一致；累计引擎 603 + UI 319 = **922 测试**
- 2026-11-14 05:5x：**M3 第三波双线启动**：P10（文件格式与规则体系：引擎注册表/检测/规则应用链 + 协议权 + 设置 UI 两新页，strings.ts 归 P10）、P6b（文件夹视图遗留小批：批量重命名/编辑菜单命令/空格标记/快速比较/复制路径/基文件夹导航——ui/views/folder-compare 独占）；与 P10 零交集设计；P11（报告/脚本处理器）排队下一波
- 2026-11-14 06:0x：P10/P6b 双双配额中断（P10 于 CLI 后设置 UI 前；P6b 于最后门禁前，冒烟已过）
- 2026-11-14 07:1x：**主会话接手收尾**：① P6b 最后门禁复验全绿（UI 361 测试/build/lint/e2e）→ 验收提交；② P10 引擎侧验收提交（workspace 699/0 全绿；修 formatAware 缺 ts(optional) 导致的 UI 编译破坏——**教训：ts-rs 导出 Option 字段必须带 ts(optional)**；golden/replay 重固化仅 formatRules 一行）；③ P10 SendMessage 续跑补设置 UI（配额窗口已过 1h+，若仍失败由主会话直接实现设置页）
- 2026-11-14 05:20：**P10/P6b 首巡**：双线活跃（文档消化段）；不干预
- 2026-11-14 05:40：**第 2 次巡检**：双线高度活跃（实现段推进中）；不干预
- 2026-11-14 12:3x：**P11 续跑完成 + V1.0 波次联合验收收官**：P11（报告 10 布局+脚本 31 命令覆盖表+report/generate RPC）验收提交（8beac7d）；**终验引擎 791/0 + UI 383 + clippy 零警 + e2e + build 全绿**；REVIEW-p11 归档（其 P0-1 崩溃重启缺陷由主会话修复：engine_start 探活清僵尸条目；P0-2/P1-10/P2 系由 P11 清偿）；**P0 需求全量闭环，进入 M4 V2.0**（P12 VFS 扩展 / P13 主题i18nShell集成 / P14 Pro 视图）；性能基准 release 复跑后台进行中
- 2026-11-14 12:4x：**V1.0 性能基准复跑（release）达标无回归**：PB-03 十万项扫描 1.549s（目标 ≤2s ✅，与 P2 基线 1.544s 持平）；PB-10 delta 序列化 23.2ms/431 万行/s（持平）
- 2026-11-14 12:44：**P12 agent 启动即遇周/月配额上限（11-18 17:52 重置，4 天后）**；用户指示继续 → **主会话直接接手 P12 开发**（zip 可写 → scheme 路由 → FTP/SFTP 分步推进）；看护任务继续值守，agent 通道恢复后复杂面（FTP/SFTP 真实服务器集成测试）交回 agent
- 2026-11-14 13:5x：**P12-a/P12-b 主会话直接开发完成并提交**（周配额中断期间自主推进）：① 可写 ZipFs（装载-内存改动-保存回写 + inflate/deflate 经 miniz_oxide MIT + write-through + zip64/加密如实 Unsupported；ai-vfs 21→40 测试）② scheme 路由（VfsPath 词法保留 scheme://、RouteFs 本地/zip 混合分发、zip:// 端到端冒烟：缺失2/异文件11/同文件0）；**主会话开发抓到并修复 persist 改造吞错误 bug**（6 写方法错误传播恢复）；workspace **810/0 全绿**；P12-c FTP/SFTP 待 11-18 配额重置交回 agent
- 2026-11-14 14:3x：**P12-c FTP 段主会话完成并提交**：FtpFs provider（suppaftp 12 tokio-rustls 纯 Rust/连接惰性复用/MLSD→LIST 回退解析/write-through STOR/递归删/set_mtime 需 MFMT 如实 Unsupported）；ftp:// 接入 RouteFs；rust-version 升 1.88（suppaftp 12 要求）；FTPS 暂缓（rustls 验证需真实服务器）、SFTP 留后半；ai-vfs 40/40、workspace 810/0；git 64 提交
- 2026-11-14 14:4x：**协调积压清偿：设置→rules 运行时接线**（P10 遗留）：TextCompareView 两处 openText 注入 effectiveRulesFor（kindDefaults compareRules + 全局 fileFormats 组装 RulesSpec；失败静默回退引擎默认）；UI 384 测试（+1）全绿；git 66 提交
- 2026-11-14 14:5x：**积压清偿：脚本 RENAME regexpr 正则改名**（P11 遗留）：regex crate + BC 模板适配层（$2_ 组名边界→${2}，防 regex 命名组吞下划线——历史 bug 有回归测试锁定）；手工复现 beta_alpha.txt 全通；script_matrix 22 测试（+1）；workspace **811/0** + clippy 零警（顺带清 4 处机械告警）；git 68 提交。积压剩余：SNAPSHOT 全量格式（P16）、FTPS/SFTP 后半（11-18 agent 恢复）

## ⏸ 暂停状态（2026-11-14 用户指令）

**用户指示：其他模型接手开发，本会话暂停，23 点后恢复。**

- 看护任务暂停期守则变更：**只巡检记录、不启动新开发、不接手收尾**（避免与其他模型冲突）；异常（构建失败/冲突）仅登记不处置
- 暂停前现场（已提交，工作树干净）：
  - P14a 图片比较引擎底座完成：ai-core 267 测试（+6 picture：容差 diff/网格块聚合/尺寸不同左上对齐/PNG roundtrip），workspace 817 全绿（2553a03）
  - **并行合并说明**：其他模型曾有 P9 picture WIP（f5777d2，267 行未接线）——本会话版本为独立重写并接线+测试覆盖，如其他模型在制品有独有逻辑请其在恢复后基于当前 HEAD 续作
  - 其他模型同期交付已在历史：P6c 双窗格（fcf065b，UI 400 测试）
- 23 点后恢复入口：从「M4 断点」继续——P12-c 后半（FTPS/SFTP）、P13、P14 视图批次、积压（SNAPSHOT 全量格式）
- 2026-11-14 19:1x：**恢复工作：吸收理解其他模型工作**（用户指示继续）：
  - **桌面壳修复**（e01099a，重大）：capabilities ACL 补 core:event:default（此前桌面壳从未跑通）、release 引擎路径改随包引擎、tauriTransport 去双重 LineBuffer（壳层已剥换行再缓冲致应答全吞）——三处根因修复+3 回归测试+变异验证（重引 bug 即红），真机 .app 实测引擎桥通
  - **P6c 双窗格**（fcf065b）：目录比较复刻 BC 双窗格（FR-DIR-001/003/020：独立选择集与活动窗格/纵向同步防回环/gutter 复制箭头/孤立留白），dirPanes 纯函数+10 测试，UI 400 测试
  - **formatSize off-by-1024**（1f4016d）：单位换算修复+边界回归
  - picture WIP（f5777d2）已由本会话收尾为 P14a 完整底座（2553a03，267 引擎测试）
  - **合并态验证：引擎 817/0 + UI 400/0 + build + e2e 全绿**（1197 测试）
  - 无其他分支/工作树遗漏（main 单分支，工作树干净）
  - 下一步：P14 视图批次（图片比较 UI 消费 picture.rs 数据面）+ P13（i18n/Shell 集成）+ P12-c 后半（agent 恢复后）
- 2026-11-14 19:3x：**P14b 引擎接线完成并提交**：picture/open RPC（协议+handler+能力位+CLI aic picture）——图片比较引擎→协议→CLI 全链路通（端到端：异图 75% 相似度/1 块/退出码 1）；workspace **822/0** + clippy 零警；golden/replay 重固化仅 pictureCompare 一行；P14 视图批次（UI 图片比较视图消费 picture/open）待续
- 2026-11-14 19:5x：**P14c 图片比较视图交付**：PictureCompareView（统计栏/容差滑杆/显示侧切换/canvas 差异块叠加+mock 演示合成图案）+ 全接线（Tab picture kind/openPictureTab/viewRegistry/主页按钮启用/CommandContext）；**顺带修复 P6c 期间 mockEngine dispatch 落穿回归**（picture/open case 插错位置吞 text/lines—— PictureCompareView 开发时测试矩阵暴露）；UI 400 全绿、e2e 过；git 71 提交

## 📋 BC 忠实度审计清单（2026-11-14 盘点；勾=达标，✗=缺口，按用户常用度排序）

| BC 功能面 | 状态 | 缺口 |
| --- | --- | --- |
| 文本比较（编辑/折叠/行内/高亮/查找/编码/概览条） | ✅ | 书签、自动换行、行详情三模式（P13/P5 后续） |
| 文件夹比较（双窗格/树/过滤/同步/操作/标记） | ✅ P6c 后 | 复制到文件夹三路径、属性对话框、回收站删除（P6 后续批次） |
| 目录双窗格 | ✅ P6c | — |
| 会话体系（树/作用域/持久化/锁定） | ✅ P4 | 三路会话格式（P11） |
| 三向文本合并（diff3 引擎/重要性/决策 + 合并视图） | ✅ 队列 3 | 冲突导航工具栏、输出直接保存为文件路径（后续批次） |
| 表格比较引擎 | ✅ P9 | **表格 UI 视图（消费 table/open|sections）——优先级高** |
| 十六进制比较引擎 | ✅ P9 | **Hex UI 视图（消费 hex/open|bytes）——优先级高** |
| 图片比较 | ✅ P14a/b/c | 闪烁模式、缩放同步、对角放大镜（后续批次） |
| 报告引擎（10 布局） | ✅ P11 | **报告对话框 UI（output-to printer/clipboard/文件）** |
| 脚本处理器（31 命令） | ✅ P11 | SNAPSHOT 全量格式（P16）、RENAME 正则部分语义 |
| CLI（aic/aicc/退出码 11-14） | ✅ P3/P11 | — |
| VFS：zip 可写 | ✅ P12 | FTPS 验证、SFTP、云（Dropbox/S3——P12 后续） |
| 主页/会话树/设置框架/快捷键引擎 | ✅ P4/P10 | 菜单栏/工具栏 UI（P13）、命令面板 |
| 编码切换 | ✅ P5 | 引擎侧非 UTF-8 解码（P10 编码检测链） |
| 桌面壳 | ✅ 打包可用 | 自动更新、安装程序（P13） |
| 剪贴板比较 | ⚠️ 半 | UI 入口（text 会话带剪贴板路径即可，小件） |
| 版本比较（exe/dll 资源） | ✗ | P14 后半（Windows-only 优先级低） |
| 注册表比较 | ✗ | P14 后半（Windows-only） |
| 媒体比较（MP3/音频） | ✗ | P14 后半（需音频解码） |

## 迭代队列（BC 忠实度驱动）
1. **表格比较 UI 视图**（table RPC 已有）✅ → 2. **Hex UI 视图** ✅ → 3. **三向合并 UI 视图** ✅ → **4. 剪贴板比较入口（下一项，小件）** → 5. 报告对话框 → 6. 文本书签/自动换行 → 7. 图片闪烁/缩放同步 → 8. P13 菜单栏/工具栏/自动更新 → 9. P14 后半（媒体/版本/注册表）

- 2026-11-14 20:1x：**P14d 表格比较视图交付**：TableCompareView（比较列网格/沟槽点 BC 语义/单元格着色/滚动分页/统计栏）+ mock table 同形 + 全接线（Tab/主页/registry/CommandContext）+ 图片/表格双视图 CSS；**迭代队列 1 完成**（队列 2：Hex UI 视图）；UI 400 全绿、e2e 过；git 72 提交
- 2026-11-14 20:3x：**P14e 十六进制比较视图交付**：HexCompareView（左右双栏 16B 行+ASCII 侧栏+逐字节状态着色+滚动分页+0x 偏移跳转+统计栏）+ mock hex/open|bytes 同形（bigint offset 适配）+ 全接线（Tab hex kind/openHexTab/viewRegistry/主页/CommandContext 联合）；**迭代队列 2 完成**；UI 400+引擎 822+clippy 零警+e2e 全过；git 73 提交
- 2026-11-14 20:4x：**P14f 三向合并视图交付 / 迭代队列 3 完成**：Merge3View（三源窗格含可隐藏的中间公共祖先窗格 + 块序列按 FR-TMRG-002 着色 + 显示过滤器五项组合语义 + 冲突块六动作采用族与批量 decide + plain/marker 输出预览/复制/导出 + 合并统计）+ 全接线（Tab `merge3` kind/Merge3TabData/openMerge3Tab/viewRegistry `text-merge` 插件/KIND_META/主页三路输入与拖放/CommandContext/mock 四方法同形）；**顺带修复 P14e 期间 `picture/open` 落穿 `hexOpen` 的 mock dispatch 回归**；**修复真实引擎空区间省略行内容字段导致的视图崩溃风险**（`paneLines` 兜底 + 回归测试）；e2e:engine 新增 merge3 四方法链路（open→sections→decide 幂等→output marker/plain）；引擎 **822/0** + UI **419**（+19）+ clippy `-D warnings` 零警 + e2e + build/lint 全绿；git 88 提交

## ⚠️ 额度紧张交接节点（2026-11-14 20:4x；队列 3 已完成）

**现场状态（工作树干净、全部门禁全绿）：**
- 引擎 **822/0** + UI **419/0**（原 400，队列 3 新增 19 测试）+ clippy `-D warnings` 零警 + `pnpm e2e:engine`（含 merge3 四方法链路）+ `pnpm build` + `pnpm lint` 全绿；git 88 提交
- **迭代队列已完成 1（表格视图）/2（Hex 视图）/3（三向合并 UI 视图）**
- **队列 4（下一接手项）：剪贴板比较入口**（小件：主页/会话树的 `text` 会话带剪贴板路径即可）
- 队列 3 交付面：消费 P7 的 `text/merge3/open|sections|decide|output`；接线模式完全照抄 picture/table/hex 三先例（Tab kind→store `openMerge3Tab`→viewRegistry→主页按钮与拖放→mock 同形→组件）；**颜色以 docs/01 FR-TMRG-002 / §2.3 为准**（仅左改青、仅右改洋红、左右不同更改=冲突红、相同更改不冲突自动合并绿、输出手动编辑黄），未采用交接草图的「蓝/绿/黄」简写
- 其余排队：队列 5 报告对话框、6 书签/自动换行、P13 菜单栏/自动更新、P14 后半（媒体/版本/注册表——需额外引擎工作）
- 已知遗留：引擎 `picture_compare` 能力位未入 handler initialize 字面量（已有字段+e2e 通过，补一行 `picture_compare: Some(true)` 即可）、SNAPSHOT 全量格式（P16）、FTPS 验证/SFTP（需真实服务器）；真实引擎空区间会省略 `baseLines`/`leftLines`（ts-rs 仍声明必填数组）——UI 已用 `paneLines` 兜底并有回归测试，后续新视图需注意
- 接手指南：读 PROGRESS 本节 + docs/01 对应 FR；接线模板参照 picture-compare/table-compare/hex-compare/merge3 四个已验收先例

## ✅ 暂停状态 2 断点已消化（2026-11-14 20:4x）

- 「暂停状态 2」登记的迭代队列 3「三向合并 UI 视图」**已完成并验收提交**（步骤 ①~⑦ 全部落地，详见 P14f 事件日志）
- 相对三个先例新增的接线点：`session/model.ts` 的 `SessionKind`/`SESSION_KINDS`/`KIND_META` 加 `text-merge`（主页大按钮由 registry 驱动）、`SettingsDialog` 的 `Record<SessionKind, string>` 同步补齐、`sliceSessionUi` 保存会话规格覆盖 `merge3`、`cleanupTabEngines` 关闭标签时释放 merge3 会话
- **队列 3 相对 FR-TMRG 的未闭环项（忠实度后续批次，验收基准仍是 docs/01 的 163 FR）**：FR-TMRG-003 冲突检测配置 UI（X 行窗口/行对齐/偏斜容差/字符级重要性——引擎 `Merge3OptionsSpec` 已备，视图当前传 `options: null`）；FR-TMRG-005「编辑 > 冲突：清除/设置冲突状态」（协议暂无 clear 动作，需引擎补）与输出直接保存为文件路径（现为复制/导出下载）；FR-TMRG-007「与输出比较 >（左/中/右）」打开文本比较会话；FR-TMRG-004 工具栏/菜单式采用命令（现为冲突决策区按钮）
- **剩余迭代队列**：4 剪贴板入口 → 5 报告对话框 → 6 书签/自动换行 → P13 菜单栏/自动更新 → P14 后半
