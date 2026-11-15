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
- [x] **M2 MVP ✅ 完成（02:1x 联合验收 + 03:5x P5 交付）**：五线交付（P1/P2/P4/P3/P5）⚠️ **2026-11-14 晚第二轮 review 更正：原「P0 需求全量覆盖」声明不成立**——SRS §6.3 P0 MVP 明列的编码自动检测、差异文件导航（FR-DIR-071）缺失，重要性编辑后被抹除；见 REVIEW.md §7.3
  - [x] P1 diff 核心（91 测试、四算法、重要性双键、增量重 diff、对拍矩阵）
  - [x] P2 目录引擎（惰性扫描、BC 过滤器全集、zip CRC、SQLite 缓存、同步状态机、10 万项 PB-03 1.54s 达标）
  - [x] P4 UI 壳会话（主页/多窗口/持久化/快捷键/重连）
  - [x] P3 CLI 全集+文本 RPC+P1 协议映射（318 引擎测试、git difftool 冒烟、协议向后兼容逐字节锚定）
  - [x] P5 文本视图完备化（214 UI 测试、可编辑/折叠/高亮/查找/编码/概览条）——**MVP 用户故事全量闭环**
- [x] **M3 前半（P3/P5）✅ 验收完成（03:5x）**
- [x] **M3 后半（P6/P6b/P7/P8/P9/P10/P11）✅ 全部验收完成（12:3x V1.0 波次联合验收）**：文件夹操作+遗留批/三向合并/同步/文件夹合并/表格hexpatch/文件格式规则/报告引擎+脚本处理器
- [x] **V1.0 波次联合验收 ✅（12:3x）**：引擎 791/0 + UI 383 + clippy 零警 + e2e + build 全绿；REVIEW-p11 深度评审清偿（P0-2/P1-10/P2 系由 P11 清偿，**P0-1 崩溃重启由主会话修复**：engine_start 探活清僵尸条目）；评审文档归档 docs/archive/REVIEW-p11.md；~~P0 需求全量闭环~~ **（2026-11-14 晚更正：不成立，见 REVIEW.md §7.3；P0 缺口=编码自动检测/差异文件导航/状态回传/重要性保持）**
- [ ] **M4 V2.0（进行中）**：P12（VFS：zip 可写 ✅ / FTP ✅ / FTPS+SFTP 待）/P13（Shell：菜单栏+工具栏 UI ✅，主题 i18n 壳集成/自动更新/安装程序待）/P14（图片 ✅ / 表格 ✅ / Hex ✅ / 三向合并视图 ✅，媒体·版本·注册表待）依次推进；**桌面发行包已可交付**（`.app`/`.dmg`，引擎随包，2026-11-14 21:4x，见文末「可体验桌面版构建」节）；协调积压随行（SNAPSHOT 全量格式、FTPS/SFTP、命令面板、设置>命令自定义页、引擎侧非 UTF-8 解码）

## 当前阶段与断点

**当前**：M4 V2.0（docs/04 P12~P14）——P0 需求全量闭环、桌面体验版（`.app`/`.dmg`，引擎随包）已可交付；第三轮深度 review（R3，输入第三方报告 `REVIEW-2026-11-15.md` 26 项 + 自查新增 R3-27/28）已收尾：可达缺陷全部修复，架构/契约/发行/工具级不确信项按「不确信就记录待决」登记（REVIEW.md §8）。**全格式演示数据集已交付**（「填充示例路径」落盘两侧各 47 个多格式文件，覆盖全部 8 类视图，经真实引擎 38 项断言验收；见文末「全格式演示数据集」节）。最新门禁与交付状态见文末「第三轮 review（R3）收尾」与「全格式演示数据集」节。

**下一步（断点恢复入口）**：
1. 待用户拍板的契约级决策（REVIEW.md §7.7 / §8.2）：CLI 退出码 BC-vs-GNU、重要性回传 RPC、合并 EOL 语义；R3 待决项——u64→bigint 类型收敛、取消粒度/出站背压架构、CSP 信任边界、跨平台发行/公证/README/LICENSE、巨型文件重构、rustfmt 全量债务/cargo-deny 执行点。
2. 平台/发布能力项（P13 后半 / 队列 9）：媒体·版本·注册表比较、主题 i18n 壳集成、命令面板、设置>命令自定义页、自动更新与公证安装程序、FTPS/SFTP、SNAPSHOT 全量格式。
3. 续作纪律：门禁全绿 → 每项独立 commit（验收写进提交信息）→ 更新本文件；push 另行授权；架构/性能变更先只读讨论再动代码；测不到就说测不到。

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
| 文本比较（编辑/折叠/行内/高亮/查找/编码/概览条/书签/自动换行） | ⚠ 队列 6 | 行详情三模式、文本合并视图自动换行（FR-TMRG-002）、**编辑后重要性被抹除（R2-N6，P1）** |
| 文件夹比较（双窗格/树/过滤/同步/操作/标记） | ⚠ P6c 后 | 复制到文件夹三路径、属性对话框、回收站删除（P6 后续批次）；**树↔平铺切换丢光文件已修（R2-10，5b7a297）**；**Update 整树复制覆写较新文件已修（R2-11，678e80d）**；**大小写变体镜像清空目标侧已修（R2-12，954bb1c）** |
| 目录双窗格 | ✅ P6c | — |
| 会话体系（树/作用域/持久化/锁定） | ✅ P4 | 三路会话格式（P11） |
| 三向文本合并（diff3 引擎/重要性/决策 + 合并视图） | ⚠ 队列 3 | 冲突导航工具栏、输出直接保存为文件路径（后续批次）、**EOL 语义与 git merge-file 不一致（R2-N4，P1，待拍板）** |
| 表格比较引擎 | ✅ P9 | **表格 UI 视图（消费 table/open|sections）——优先级高** |
| 十六进制比较引擎 | ✅ P9 | **Hex UI 视图（消费 hex/open|bytes）——优先级高** |
| 图片比较 | ✅ 队列 7 | 对角放大镜、光标像素缩略图与 RGB、透明棋盘格、颜色自定义、旋转/翻转（后续批次） |
| 报告引擎 + 报告对话框 | ✅ 队列 5 | HTML 外部样式表/print-color 打印渲染细节、include-file-links 逐文件报告链接（见末尾最新状态节） |
| 脚本处理器（31 命令） | ✅ P11 | SNAPSHOT 全量格式（P16）、RENAME 正则部分语义 |
| CLI（aic/aicc/退出码 11-14） | ⚠ P3/P11 | **退出码 0/1/2 语义与 BC 相反（BC：1=Binary same、2=Rules-based same、100=error；我们：1=diff、2=error），76 处调用点，契约级待拍板（R2-N1）** |
| VFS：zip 可写 | ✅ P12 | FTPS 验证、SFTP、云（Dropbox/S3——P12 后续） |
| 主页/会话树/设置框架/快捷键引擎/菜单栏/工具栏 | ✅ 队列 8 | 命令面板、主题 i18n 壳集成、工具栏自定义设置页（FR-UI-003 后半） |
| 编码切换 | ⚠ P5（仅 UI 层） | **引擎 `read_file_text` 只接受 UTF-8，GBK 等直接报错（R2-N2，P0 缺口：SRS §6.3 明列编码自动检测）**；UI 切换不改变引擎解码 |
| 桌面壳 | ✅ 打包可用 | 自动更新、安装程序（P13）、签名与公证（当前 ad-hoc 签名） |
| 桌面发行包（.app/.dmg 双击即用、引擎随包） | ✅ 21:4x | Windows/Linux 打包路径未验证（脚本仅 macOS）、公证（notarization）、自动更新 |
| 剪贴板比较 | ✅ 队列 4 | 系统托盘常驻、全局快捷键注册、FR-CLIP-003 选项页（Web 内以面板/命令替代，见末尾最新状态节） |
| 版本比较（exe/dll 资源） | ✗ | P14 后半（Windows-only 优先级低） |
| 注册表比较 | ✗ | P14 后半（Windows-only） |
| 媒体比较（MP3/音频） | ✗ | P14 后半（需音频解码） |

## 迭代队列（BC 忠实度驱动）
1. **表格比较 UI 视图**（table RPC 已有）✅ → 2. **Hex UI 视图** ✅ → 3. **三向合并 UI 视图** ✅ → 4. **剪贴板比较入口** ✅ → 5. **报告对话框** ✅ → 6. **文本书签/自动换行** ✅ → 7. **图片闪烁/缩放同步** ✅ → 8. **菜单栏/工具栏 UI** ✅（P13 自动更新/主题 i18n 未纳入，见末尾）→ 9. P14 后半（媒体/版本/注册表，需平台/解码器支持）

- 2026-11-14 20:1x：**P14d 表格比较视图交付**：TableCompareView（比较列网格/沟槽点 BC 语义/单元格着色/滚动分页/统计栏）+ mock table 同形 + 全接线（Tab/主页/registry/CommandContext）+ 图片/表格双视图 CSS；**迭代队列 1 完成**（队列 2：Hex UI 视图）；UI 400 全绿、e2e 过；git 72 提交
- 2026-11-14 20:3x：**P14e 十六进制比较视图交付**：HexCompareView（左右双栏 16B 行+ASCII 侧栏+逐字节状态着色+滚动分页+0x 偏移跳转+统计栏）+ mock hex/open|bytes 同形（bigint offset 适配）+ 全接线（Tab hex kind/openHexTab/viewRegistry/主页/CommandContext 联合）；**迭代队列 2 完成**；UI 400+引擎 822+clippy 零警+e2e 全过；git 73 提交
- 2026-11-14 20:4x：**P14f 三向合并视图交付 / 迭代队列 3 完成**：Merge3View（三源窗格含可隐藏的中间公共祖先窗格 + 块序列按 FR-TMRG-002 着色 + 显示过滤器五项组合语义 + 冲突块六动作采用族与批量 decide + plain/marker 输出预览/复制/导出 + 合并统计）+ 全接线（Tab `merge3` kind/Merge3TabData/openMerge3Tab/viewRegistry `text-merge` 插件/KIND_META/主页三路输入与拖放/CommandContext/mock 四方法同形）；**顺带修复 P14e 期间 `picture/open` 落穿 `hexOpen` 的 mock dispatch 回归**；**修复真实引擎空区间省略行内容字段导致的视图崩溃风险**（`paneLines` 兜底 + 回归测试）；e2e:engine 新增 merge3 四方法链路（open→sections→decide 幂等→output marker/plain）；引擎 **822/0** + UI **419**（+19）+ clippy `-D warnings` 零警 + e2e + build/lint 全绿；git 88 提交
- 2026-11-14 20:3x：**P14g 剪贴板比较入口交付 / 迭代队列 4 完成**：`clipboard://N` 规格（FR-CLIP-004）+ 剪贴板历史（相邻去重/最小长度/条数与 MB 上限且至少保留最近两条，FR-CLIP-003）+ `Ctrl+Alt+C` 比较最近两条（FR-CLIP-002，占位命令替换为真实实现）+ 历史面板（列表/另存为/删除/与相邻条目比较/用文本编辑打开，FR-CLIP-001）+ 文本会话「与剪贴板比较」命令 + 主页入口；**复用既有 Tauri `settings_write` 把剪贴板文本物化为临时文件**，未改 Rust 协议面；mock `text/open` 读同一历史单例使浏览器演示即真实剪贴板内容；新增 `ClipboardHistoryPanel` / `clipboardActions` / `clipboardStore` 与 15 个测试；引擎 822/0 + UI **434**（+15）+ clippy 零警 + e2e + build/lint 全绿

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

## 看门狗（2026-11-14 20:1x 用户指令）

**用户指令**：每完成一项功能汇报一次进度 + commit + 更新文档，直至最终实现设计目标；之后多轮 self-review 并修复发现的问题；设置看门狗每 10 分钟检查当前任务是否继续，遇服务端停止响应/网络中断等异常自动继续。

**双层机制（实测落地）**：
- 本地 10 分钟层 `scripts/watchdog.sh`：`nohup bash scripts/watchdog.sh > target/watchdog/stdout.log 2>&1 &` 后台常驻，每 600 秒检查 —— 最近 git commit 时间、`ui/src`/`engine`/`cli` 源码 mtime、活跃的 `cargo test|clippy|build` / `vitest` / `pnpm test|build|lint|e2e` / `ai-engine` 进程（已排除常驻 `pnpm dev` 与 IDE 宿主，否则停滞判定永远失效）、网络两次探活（apple.com → baidu.com）、工作树未提交改动；停滞阈值 30 分钟（`WATCHDOG_STALE` 可覆盖）且带 30 秒二次复验，避免主会话读文档/思考期误报。心跳写 `target/watchdog/heartbeat.json`，异常追加 `target/watchdog/SIGNAL.md`，巡检流水 `target/watchdog/check.log`（均落在已 gitignore 的 `target/` 下）。
- AI 接手层 IDE automation `ai-compare-10`（ACTIVE，`FREQ=HOURLY;INTERVAL=1`，cwd = 本工作区）：每小时先读心跳/信号；`SIGNAL.md` 非空、心跳超过 20 分钟未更新、或信号含网络中断/服务端停止响应/会话中断 → 立即从 PROGRESS 断点自动继续开发（不必等下一轮），必要时重新拉起守护进程，处理完清空信号。心跳正常则只巡检登记、不重复开发。
- **调度粒度限制（如实登记）**：IDE automation 的 RRULE 仅支持 HOURLY/DAILY/WEEKLY，实测 `BYMINUTE=0,10,20,30,40,50` 与多值 `BYHOUR` 均不展开（`nextRunAt` 恒为创建时间 +1h 或固定日边界），故 **10 分钟检查由本地守护承担，AI 自动继续最长有 1 小时接续延迟**；若后续 IDE 支持 minutely，可把 automation 改为 `FREQ=MINUTELY;INTERVAL=10` 并退役本地守护。
- 纪律：看门狗禁止 `git merge` 其它分支、禁止 force push、禁止 push（用户只授权 commit）；异常接手后仍须门禁全绿才提交；每轮巡检结论追加到本文件并单独提交。
- 停止：`kill "$(cat target/watchdog/pid)"`；重启：先 kill 再按上面的 nohup 命令拉起。

## 最新状态（2026-11-14 21:1x；覆盖上文交接节点的旧数字）

- 现场：工作树干净、全部门禁全绿 —— 引擎 **822/0**、UI **470/0**、clippy `-D warnings` 零警、`pnpm e2e:engine`（含 merge3 四方法链路 + report/generate 分块流）、`pnpm build`、`pnpm lint` 零警告；看门狗守护存活（10 分钟心跳 + 每小时 AI 接手）。多轮自审与修复清单见文末「多轮自审」节。
- 迭代队列：**1~8 的 UI 交互面已完成**（8 仅菜单栏/工具栏）；**队列 9 与 P13 后半为平台能力项**：媒体比较（需音频解码）、版本比较（Windows exe/dll 资源）、注册表比较（Windows 实时注册表）、自动更新与安装程序（需签名/发布基础设施）、主题 i18n 壳集成与命令面板（FR-UI-003 后半）——均需额外引擎或平台/发布能力，非本环境可闭环，已如实登记交由后续批次/看门狗续作。
- **P14k 菜单栏与工具栏交付（队列 8）**：`MenuBar`（五个菜单按命令 category 分组、when 门控灰显、显示默认键位、帮助项、点击外部关闭）+ `ToolBar`（收藏命令、右键循环 大按钮/紧凑/隐藏、`app.toolbarLayout` 菜单项可在隐藏态恢复）+ `menuModel.ts` 纯函数（分组/门控/收藏/布局循环/短标签）+ UiSlice 布局状态 + 4 个测试。
- **队列 8 相对 FR-UI 的未闭环项（忠实度后续批次）**：设置 > 命令页（按视图类型控制命令显示、快捷键自定义与冲突提示、工具栏收藏编辑、导出 .bcpkg）、命令面板、菜单栏原生系统菜单（macOS）、主题 i18n 壳集成。
- **P14j 图片闪烁与缩放同步交付（队列 7）**：`pictureViewModel.ts` 纯函数（缩放钳制/1.25 步长/适合/实际像素、FR-PIX-003 较小图自动缩放几何、FR-PIX-002 闪烁相位与三角波混合、箭头键差异偏移、平移钳制与同步变换串）+ 视图重写为左右双窗格共用 `ZoomState`（放大/缩小/适合/实际像素、Ctrl+滚轮、拖动平移同步）+ 闪烁/混合节拍 + 自动缩放开关 + 偏移标签/重置 + 差异块叠加；9 个测试。
- **队列 7 相对 FR-PIX 的未闭环项（忠实度后续批次）**：对角放大镜与光标像素缩略图/RGB（FR-PIX-004）、透明棋盘格、容差模式颜色自定义与颜色替换表（FR-PIX-002）、旋转/翻转（FR-PIX-003）、不匹配范围模式（黑/黄亮度）、`picture-report` 报告布局。
- **P14i 书签与自动换行交付（队列 6）**：`bookmarks.ts` 纯函数（编号 0-9 切换/指定/清除/上一·下一循环导航/标签）+ CM6 行装饰黄色编号标记（`data-bookmark`）+ 工具栏全套操作（换行按钮/切换书签/上一·下一/清除/指定编号下拉/编号:行号显示）+ 自动换行经容器 class 覆盖 `.cm-content` 长行折行 + 命令总线 `textViewBus` + 5 条视图命令与 BC 惯例键位（`Ctrl+F2` 切换 / `F2` 下一个 / `Shift+F2` 上一个，构建期零键位冲突）+ 9 个测试。
- **队列 6 相对 FR-TXT 的未闭环项（忠实度后续批次）**：自动换行当前用 CSS 覆盖 `.cm-content`（非 CM6 `lineWrapping` 扩展，几何同步依赖 CM ResizeObserver）；文本合并视图的自动换行（FR-TMRG-002）、行详情三模式（FR-TXT-030）尚未接入；书签为视图内状态，未随命名会话持久化。
- **P14h 报告对话框交付（队列 5）**：`ReportDialog`（布局按会话类型分派 10 布局族/布局级选项：display-*、dirDisplay 12 标志、可选列 9 项、patchStyle、delimiter、color、wrap、wrapColumns、lineNumbers、includeFileLinks、标题自定义）+ 输出目标（打印预览 / HTML 文件 / HTML 剪贴板 / 纯文本文件 / 纯文本剪贴板 / 浏览器查看 / 仅预览）+ 分块流拼接与预览；`report/generate` 引擎封装（u64 分块偏移收窄为 number）+ mock 同形实现（布局族/选项回显/分块）+ 状态栏入口 + `app.report` 命令 + e2e 真实引擎 summary/patch/分块/410 断言；新增 7 个测试。
- **队列 5 相对 FR-RPT 的未闭环项（忠实度后续批次）**：HTML 外部样式表与 html-custom 主题、print-color/print-mono 与 portrait/landscape 打印渲染细节、include-file-links 内嵌逐文件报告链接、strikeout-left/right-diffs（interleaved）、各会话类型的会话菜单内独立报告命令（当前为状态栏/命令统一入口）。
- **队列 4 相对 FR-CLIP 的未闭环项（忠实度后续批次）**：FR-CLIP-001 系统托盘常驻与后台自动监视剪贴板（Web 内为手动「捕获当前剪贴板」+ 历史面板）；FR-CLIP-002 全局快捷键注册（当前为应用内 `Ctrl+Alt+C`）；FR-CLIP-003 选项页（启动运行/键位开关/格式掩码/最小长度/条数与 MB 上限，当前为代码默认值 `DEFAULT_CLIP_LIMITS`）；FR-CLIP-004「文件 > 从剪贴板打开」直接加载到选定窗格（当前为与当前会话左文件新开文本比较）、「编辑 > 将选择与剪贴板比较」（当前无编辑器选区注入）、命令行 `clipboard://` 规格（当前仅 UI 内部使用，引擎 VFS 未注册该 scheme）。
- 上文「⚠️ 额度紧张交接节点」的门禁数字（UI 400/419/434）与「下一接手项 = 队列 3」为历史快照，以本节为准。

## 多轮自审（2026-11-14 21:0x；用户指令「多次 review 自己修复发现的问题」）

- **轮 1（自查 + 门禁）**：① `merge3/open` / `picture/open` 会话已存在（411）未重建，React StrictMode 双挂载与图片容差滑杆重比较会失败 → 新增 `openPictureIdempotent` / `merge3OpenIdempotent`（幂等 close 后重开）；② `clipboardStore.registerText` 在空历史且被过滤时返回无效 `clipboard://0`；③ 报告 HTML 布局误传 `wrap=character`（FR-RPT-001 仅 none/word）→ 归一为 word；④ MenuBar 命令使用菜单展开期间的过期 ctx → 点击时重建上下文。
- **轮 2（独立只读审查 + 门禁）**：P1 `ToolBar` 未订阅 `activeWindowId`（切换活动窗口后命令 when/run 用错标签）→ 补订阅；P2 历史已满时 `add()` 因裁剪误返回 null（改用尾部新 id 判断）、`registerText('')` 返回不存在规格（绕过 appendClip 强制插入）、`autoScaleGeometry` 两侧分别取 max 破坏等比（改按统一目标宽度对齐）、`deliverReport` 打印/浏览器把纯文本当 HTML 写入（新增 `escapeHtml` / `reportHtmlBody`）、容差快速拖动 close/open 竞态（请求序号 + 串行队列，只应用最后一次结果）。
- **轮 3（安全/健壮性自查）**：报告标题 HTML 敏感字符清洗、菜单/工具栏命令 async 异常未捕获（`runCommandSafely` 统一捕获并提示）、多窗口 `textViewBus` 注销误清空其它视图（改为注册栈 + token disposer）。看门狗脚本复核：无 eval 注入、pid 文件仅数字、日志落在已忽略的 `target/`。
- **回归测试**：411→close→open 链路、历史满 `add`、空文本 `registerText`、HTML 转义/包装/标题清洗、命令异常捕获、命令总线注册栈等；UI 测试 463 → **470**。
- **最终门禁**：引擎 **822/0**、UI **470/0**、clippy `-D warnings` 零警、`pnpm e2e:engine`（含 merge3 四方法与 report/generate 分块链路）、`pnpm build`、`pnpm lint` 零警告；看门狗守护进程存活（10 分钟心跳 + 每小时 AI 接手）。
- **边界（如实登记）**：队列 9 与 P13 后半属平台/发布能力项（Windows exe/dll 资源与实时注册表比较、音频解码、签名更新与安装程序、主题 i18n 壳集成、命令面板、设置 > 命令自定义页），需额外引擎或发布基础设施，非本环境可闭环；已登记为后续批次，看门狗据此续作。

## 最终交付状态（2026-11-14 21:1x）

- **HEAD = `6bd20de`**，main 单分支，工作树干净；本会话共 8 次提交（看门狗 + 队列 4~8 + 自审修复 + 状态登记），每项功能均单独 commit 并同步更新本文件。
- **最终门禁复验（提交后重跑）**：`cargo test --workspace` **822/0**、`cargo clippy --workspace --all-targets -- -D warnings` 零警、`pnpm test` **470/0**（47 文件）、`pnpm e2e:engine` 全过（含 merge3 四方法与 report/generate 分块链路）、`pnpm build`、`pnpm lint` 零警告。
- **看门狗**：IDE automation `ai-compare-10` ACTIVE（`FREQ=HOURLY;INTERVAL=1`，cwd 本工作区）；本地 `scripts/watchdog.sh` 后台存活，每 600 秒心跳 `target/watchdog/heartbeat.json`、异常信号 `target/watchdog/SIGNAL.md`、巡检流水 `target/watchdog/check.log`。IDE 调度最小粒度 1 小时，故 10 分钟检查由本地守护承担、AI 自动继续最长 1 小时接续延迟（已如实登记）。
- **交付面**：迭代队列 1~8 的 UI 交互面全部闭环（1~3 由前序会话完成，4 剪贴板 / 5 报告对话框 / 6 书签与自动换行 / 7 图片闪烁缩放 / 8 菜单栏工具栏由本会话完成），三轮自审修复 12 项真实缺陷。
- **后续断点**：队列 9 与 P13 后半（平台/发布能力项）、各队列已登记的未闭环 FR（详见各节）；看门狗遇异常自动从本节断点续作，续作时仍须「门禁全绿 → commit → 更新本文件」。

## 可体验桌面版构建（2026-11-14 21:4x）

用户要求「build 一个可用的版本体验」→ 产出可双击运行的 `.app` 与 `.dmg`，并修复两个真实可用性缺陷。

**打包产物**
- `scripts/build-app.sh [--dmg]`：引擎 release → 前端 → `tauri build --bundles app` → **随包放入引擎**（`ai-engine` 复制进 `ai-compare.app/Contents/MacOS/`）→ 重新 ad-hoc 签名；`--dmg` 时用 `hdiutil create` 以该 `.app` 为源打 DMG。
- 产物：`ui/src-tauri/target/release/bundle/macos/ai-compare.app`、`.../bundle/dmg/ai-compare_0.1.0_aarch64-apple-darwin.dmg`。
- **为何不用 `tauri build --bundles dmg`**：① 它会重建 `bundle/macos/*.app`，把随包放入的引擎覆盖掉；② 其 `bundle_dmg.sh` 需挂载/卸载卷，无头环境下以 `hdiutil couldn't unmount "disk4" - 资源忙` 失败。改用 `hdiutil create -srcfolder`（不挂载卷）。

**缺陷 1：桌面版「填充示例路径」必报「根路径不存在（left）: /demo/project-v1」**
- 根因：`/demo/project-v1` 是 `strings.ts` 的**演示引擎占位路径**——内置 mock 引擎不触碰文件系统，任意路径都渲染合成目录树；而桌面版走真实引擎，`dir/open` 真的扫文件系统 → `ai_dir::DirScanError::RootNotFound`（`engine/ai-engine/src/mapping.rs:29`）。引擎报错如实，缺陷在 UI 把 mock 占位路径喂给真实引擎。
- 修复：新增 `ui/src/lib/demoSeed.ts`（演示文件清单 + `seedDemoDirs` 落盘，状态覆盖相同/不同/仅左/仅右）；`engineProcess` 导出 `isMockEngineActive()`；主页按钮按环境分流——mock 仍填占位路径，真实引擎改为在**应用数据目录**（`<baseDir>/demo/project-v1|project-v2`）落盘真实演示目录再填入。写前先删同名文件与 `.bak`（`settings_write` 对已存在目标会留 `.bak`，会污染目录 diff）。
- 实证：真实引擎对含空格路径（`Application Support`）正常；`aic diff` 退出码 1 且差异正确；JSON-RPC `dir/open` 返回 8 行，状态分布 `same 2 / contentDiff 2 / leftOnly 2 / dirDiff 1 / rightOnly 1`。

**缺陷 2：发行包缺引擎 → 桌面用户被指引去「仓库根执行 cargo build」**
- 根因：Tauri bundle 不携带引擎，而壳在 release 下按「与可执行文件同目录」解析；DMG 由 Tauri 生成时 app 被重建，引擎丢失 → 用户从 DMG 启动即报「未找到引擎可执行文件」。
- 修复：打包流程固化「随包放入引擎 + 重签名」（新增可执行文件会使原签名失效）；`ui/src-tauri/src/lib.rs` 的缺引擎文案按 `debug_assertions` 分流，release 改为「当前安装包不完整，请重新获取完整安装包」。
- 实证：从 DMG 挂载启动 → `ai-engine --stdio` 子进程正常拉起（进程路径在 `.app/Contents/MacOS/` 内）。

**门禁**：`pnpm test` **477/0**（48 文件，+7 新增 demoSeed 测试）、`pnpm build`、`pnpm lint` 零警告、`pnpm e2e:engine` 全绿、`ui/src-tauri` 独立 workspace 的 `cargo clippy --all-targets -- -D warnings` 零警。

**提交**：`fe04624` M4-体验版: 桌面版打包脚本(引擎随包+DMG)与两项可用性缺陷修复。

**2026-11-15 00:0x reinstall**：第二轮 review 三项修复（R2-10/11/12）后重打包并覆盖 `/Applications/ai-compare.app`；验签通过；在安装版二进制上实测平铺切换/刷新/Update/镜像四项行为正确（原始输出见 REVIEW.md §7.9）。旧包保留于 /tmp 未删除。

**架构登记**：docs/03 追加 ADR-011（桌面发行形态：引擎随包分发与壳内引擎解析顺序）；`ui/README.md` 追加「桌面发行包」章节（打包命令、产物路径、随包机制、DMG 打包取舍）。

## 最新交付状态（2026-11-14 21:5x；覆盖上文「最终交付状态 21:1x」的数字）

- **HEAD = `fe04624`**（main 单分支，工作树干净；本节文档更新另计一次提交）。
- **门禁复验（提交后重跑）**：引擎 `cargo test --workspace` **822/0**、UI `pnpm test` **477/0**（48 文件）、`pnpm build`、`pnpm lint` 零警告、`pnpm e2e:engine` 全绿、`ui/src-tauri` 独立 workspace `cargo clippy --all-targets -- -D warnings` 零警。
- **可体验产物**：
  - `.app`：`ui/src-tauri/target/release/bundle/macos/ai-compare.app`（双击即用，引擎在 `Contents/MacOS/ai-engine`）
  - `.dmg`：`ui/src-tauri/target/release/bundle/dmg/ai-compare_0.1.0_aarch64-apple-darwin.dmg`（已实测挂载启动 → `ai-engine --stdio` 正常拉起）
  - 一键重打包：`scripts/build-app.sh [--dmg]`
- **体验路径**：主页「填充示例路径」→ 真实引擎下自动在 `~/Library/Application Support/ai-compare/demo/` 落盘真实演示目录 → 「开始比较」得真实目录 diff；或直接把两个真实目录/文件拖入窗口。
- **已知边界（如实登记）**：签名仍为 ad-hoc（无公证/notarization）；打包脚本仅 macOS（Windows/Linux 的 `.exe`/`.deb` 路径未验证）；发行包内引擎缺失时的兜底是前端降级横幅（可切演示引擎），不自动从网络补齐。
- **后续断点**：队列 9 与 P13 后半（平台/发布能力项：媒体/版本/注册表比较、主题 i18n 壳集成、命令面板、设置>命令自定义页、自动更新与公证安装程序）、各队列已登记的未闭环 FR（详见各节）；看门狗遇异常自动从本节断点续作，续作时仍须「门禁全绿 → commit → 更新本文件」。

## 深度 Review（2026-11-14 22:0x~23:0x；用户指令「做一次全面的深度 review」）

**评审报告**：仓库根 `REVIEW.md`（19 项已修 + 21 项登记 + 风险矩阵 + 变异验证矩阵 + 排期建议）。

**方法**：静态走查 + 双路并行子代理深挖（引擎协议/CLI 边界 + UI 会话与进程管理）+ **变异验证**（逐条把修复回退，确认回归测试确实变红）。

### 已修 19 项（全部含回归测试；7 项经变异验证）

**引擎侧 9 项**
1. **P0 引擎 EOF 后进程永挂死**：`pump` 常驻任务持有出站通道克隆 → `writer.await` 永久阻塞（只能 kill -9）。修复：退出前 `cancel_all_dir_sessions` 取消全部 DirModel 令牌，pump ≤150ms 内退出。
2. **P0 协议边界无 panic 隔离**：注释声称有、实现全仓 `catch_unwind` 命中 0 → handler panic 时该请求**永不应答**（UI 只能等超时）。修复：`tokio::spawn(AssertUnwindSafe(..))` → `-32603`。
3. `"id": null` 被误判为非法信封 → `RequestId` 增补 `Null` 变体（JSON-RPC 2.0 §4）。
4. 解析失败/信封非法应答 id 固定为 `0` → 与真实 id=0 撞号（幽灵应答）→ 改回 `null` / 回显原 id。
5. 不校验 `jsonrpc` 版本 → 非 `"2.0"` 回 `-32600`。
6. **P1 `text/merge3/open` 非幂等 → 标签重挂载清空全部已提交冲突决策** → 引擎侧同参数幂等返回（保留 decisions/rev）。
7. `aic patch` 应用失败退出码 1 → 2（错误，非「文件不同」）。
8. `aic patch --dir` 的 `AlreadyApplied`/`SkippedBinary` 误判为差异 → 按「执行」语义重定义（失败/已应用 → 2，其余 → 0）。
9. `--conflict-exit-code 0` 未校验 → 显式拒绝（破坏「0 = 相同」不变量）。

**UI 侧 9 项**
10. 横幅「重试」**不发起连接**（只清状态）→ 补 `getEngineClient()`。
11. 关闭标签未释放 picture/table/hex/sync/merge 引擎会话（引擎侧只增不减 → 泄漏）→ 全覆盖清单。
12. `rebindSession` 只覆盖 folder/text → 崩溃后其余标签持失效句柄（410/411）→ 全覆盖。
13. resync 降级重放后新句柄未登记 `trackedSessions` → 第二次崩溃不再恢复。
14. 崩溃重启后旧 Tauri 监听器不回收（每次崩溃净增 3 个 + 重复投喂 stdout）→ `old?.close()`。
15. 单个通知订阅者抛错中断整条分发链 → 逐个 try/catch（client.ts + dir/delta）。
16. Hex 视图 `loading` 陈旧闭包恒为 false → 并发同 offset 拉页（重复行 + 偏移漂移）→ 改用 ref 在途标记。
17. Hex 视图判错能力位（`tableCompare` → `hexCompare`）。
18. 关闭窗口/关闭其他标签静默丢弃脏的命名会话（违反 FR-SES-001）→ 先走关闭确认。

**测试基建 1 项**
19. **单跑测试目标时引擎二进制不重建 → 回归测试形同虚设**（本次变异验证踩中：禁用修复后用例仍全绿）→ `ensure_engine_fresh()` 按 mtime 现场重建。

### 门禁（修复后全量复跑）

- 引擎 `cargo test --workspace` **829/0**（+7）、`cargo clippy --workspace --all-targets -- -D warnings` 零警
- UI `pnpm test` **484/0**（49 文件，+7）、`pnpm lint` 零警告、`pnpm build` 通过、`pnpm e2e:engine` 全绿

**新增测试**：`tests/protocol_conformance/protocol_test.rs`（7 条：EOF 退出 ×2、id=null、解析错误 id=null、信封非法回显 id、jsonrpc 版本、merge3 幂等保决策）、`ui/src/shell/sessionLifecycle.test.ts`（7 条）。

### 未修登记（21 项，见 REVIEW.md §4）

- **P1 10 项**：ts-rs 导出与 serde 语义脱节（169 处 `Option`）、u64→bigint 与线上 number 不符、原子写缺 fsync + `.bak` 由 rename 产生、临时名按 PID + 持久化无串行化、mock 与真引擎在 `dir/merge*` 三方法分叉、`deleteFile` 不清 `.bak` 导致删除的会话复活、`trackSession` 未覆盖 picture/table/hex/merge3、合并决策仍为视图内状态、panic 隔离缺回归、**无 CI**。
- **P2 13 项**：能力位 `cancel: true` 过度承诺、工具链未钉版本、MSRV 不一致 + 壳层脱离 workspace 覆盖、cargo-deny 与 rsproxy 镜像可能冲突、出站通道无背压、`--record` 静默失效、取消注册表用 `std::sync::Mutex`、无行长上限、客户端只接受 number id、`aic report` 退出码语义待确认、书签/换行未持久化、hex 跳转索引算式可疑、`deleteFile` 调用面。
- **P3 6 项**：并发连接竞态、`TrackedSession.kind` 收窄、通知方法名与请求同名、Python 语料生成器未声明依赖、增量 diff 边界吸收缺性质测试、行内 diff 超时降级语义待核实。

### 后续断点（看门狗续作入口）

优先批次：① P1-07+P1-08（崩溃恢复登记 + 合并决策持久化，与本次 F-11/F-12 同族）→ ② P1-05（mock/真引擎分叉，直接影响 UI 测试可信度）→ ③ P1-09（panic 隔离回归）→ ④ P1-03+P1-04（持久化原子性）→ ⑤ P1-01+P1-02（协议类型可信度）→ ⑥ P1-10（CI）。

- 2026-11-15 00:02：**暂停期巡检（00:02，用户暂停指令仍生效——不推进）**：其他模型持续开发中——暂停后新增约 10+ 提交（sync 两处数据丢失修复 R2-11/R2-12、过滤器切换丢文件 R2-10、symlink 逃逸 P0 安全修复、hex 跳转/目录掩码/XML 报告/脚本层多项 review 清偿）；工作树仅 REVIEW.md 未跟踪（其他模型的评审文档，归属其所有者）；本会话零动作，保持暂停
- 2026-11-15 00:1x：**第二轮深度 review 收尾 + 本机安装版更新（REVIEW.md 所有者会话）**：
  - **用户实测缺陷修复**：「树形改平铺后界面无任何文件、切回树形也不恢复」→ 根因=UI 显示类切换每次附带空 nameFilter，`update_filters` 无条件重扫摧毁已展开子树。修复=过滤器判等 no-op + 强制重扫显式入口（`DirModel::refresh()`、`dir/filter` 新增 `rescan` 参数）。提交 `5b7a297`
  - **该修复暴露的两条借道重扫回归**：① `dir/execute` 落盘后原借 `update_filters(None,None)` 重扫 → 被 no-op 化，**由 p9_e2e 门禁真实抓红**后改调 `refresh()`；② 右键「刷新」原调 `applyFilters()` → 改下发 `rescan:true`（读调用链发现，视图层无测试覆盖）
  - **同步计划层两项数据丢失**：Update 预设整树复制目录行覆写较新子文件（`678e80d`）；大小写不敏感卷镜像先 copy 后 delete 清空目标侧且 exit 0（`954bb1c`，delete 提前到 copy 前，两种卷语义均正确）
  - **门禁**：引擎 **842/0**、clippy 零警、UI **485/0**（49 文件）、eslint/tsc+vite build/e2e:engine 全绿；三项修复均双向变异验证（含过度修复变异）
  - **安装**：`bash scripts/build-app.sh --dmg` 重打包 → 验签通过 → 覆盖 `/Applications/ai-compare.app`（旧包移至 /tmp 保留）。**在安装版二进制上实测**：平铺切换 upserts 含子树文件且 removals 空、切回树形展开态保留、`rescan:true` 刷出外部新建文件；Update 计划仅 `sub/h.txt` 且右较新文件保留 `NEWER-RIGHT`；镜像计划 delete 先于 copy、右盘终态 `A.txt=LEFT`。GUI 启动渲染正常（引擎子进程随会话拉起）
  - **文档校准**：本文件 M2/V1.0 的「P0 全量闭环/覆盖」声明更正为不成立；BC 审计表 5 行改 ⚠ 并登记缺口；REVIEW.md §7 补 R2-10/11/12、子 agent 两条声称的逐条裁定（原始机理均被否证、相邻真缺陷成立）、门禁与变异矩阵
  - **待用户拍板（REVIEW.md §7.7）**：CLI 退出码 BC-vs-GNU（76 调用点）、重要性编辑后保持（新 RPC vs TS 复刻）、合并 EOL 语义（vs git merge-file）

## 第三轮深度 Review（R3）收尾 + 体验版重装（2026-11-15 09:1x；用户指令「对 REVIEW-2026-11-15.md 指出的问题确认自主决策全部修复直至完成」）

**评审报告**：第三方 `REVIEW-2026-11-15.md`（26 项 R3-01…R3-26，未跟踪，归属其所有者会话）+ 自查补 R3-27/R3-28，共 28 项。逐项处置全表（含提交 hash）见 `REVIEW.md §8.1`，待决证据 + 修法 + 为何后置见 §8.2，门禁复验见 §8.4。

**处置结论（截至代码 HEAD `7c4f809`）**：28 项中 **22 项完全修复**（R3-01…R3-20、R3-27、R3-28）、**5 项部分修复**（R3-21/22/23/24/26：可达子项已修，架构/契约/工具/法律子项登记待决）、**1 项完全待决**（R3-25 巨型单文件重构）。
- **P0 数据安全**：两条符号链接根外写入（递归复制 `96f564a`、补丁应用 `5ceed84`）。
- **可达功能缺陷全修**：平铺/手动对齐行地址 + 目录扫描失败误判相同（`5dde3f5`）、文本编辑真实落盘 + 脏保护（`3acd46c`/`9841b7d`）、三向合并末尾换行/输出落盘/文件夹合并真实 RPC（`6493247`/`105e786`/`a604bc2`）、会话保存重开 + 崩溃恢复全 7 类（`457e43b`/`892bd2a`）、table/hex 411/410 竞态（`71b15f8`/`b5e24fa`）、zip 回写失败误报成功 + 设置原子耐久（`23f43d0`）、GUI RouteFs/编码权威/目录内容比较三方式（`0bb022a`/`d7722d7`/`652341a`）、父子会话闭环 + 差异导航（`861b53a` 等）、编辑后重 diff 重要性不抹平（`bec61f6`）、文本首开丢规则（`77fa72d`）、切标签丢目录视图状态（`7c4f809`）。
- **待决项均非可达缺陷**（门禁全绿、真实链路正确）：R3-21(b) u64→bigint 类型 hygiene、R3-22(c) 取消粒度 + (d) 无界 channel 背压（架构）、R3-23 CSP（无注入汇且无法在真实 WKWebView 验证）、R3-24 跨平台/公证/README/LICENSE（需用户凭据与许可证拍板）、R3-25 巨型文件重构（纯结构、风险高收益零）、R3-26 rustfmt 全 workspace + cargo-deny（机械 churn / 装工具）。按「架构变更先只读讨论」「测不到就说测不到」「不擅自装工具/盲改契约」记录待用户拍板（§8.2）。

**formats/rules 接线闭环**：R3-27 文本首开丢比较规则（壳预开无规则 → 视图 411 复用）已修，rules/formatRules 在 open→编辑→重 diff→保存全链路贯通；mergeSave/dirMerge 真实 RPC 经 e2e:engine 实测。

**门禁（R3-28 提交后全量复验，详见 §8.4）**：引擎 `cargo test --workspace` **987/0**（3 ignored）+ clippy 零警；壳独立 workspace **15/0**（含 confine_within 5 测试）+ clippy 零警；协议 drift `generated_bindings_match_ui` 通过（192 .ts 逐字节一致）；UI `tsc -b` 0 错、`vitest` **729/0**（72 文件）、`lint`/`build`/`e2e:engine`/`format:check` 全绿。R3-28 组件 rehydrate（视图层零 `.test.tsx`）由真实浏览器 headless Chrome + CDP **10/10 断言 + 双向变异**补足（savedView=null → 4 条断言变红，证非空跑）。

**体验版重装（用户指令「编译最新代码在我本机安装上方便我体验」）**：`bash scripts/build-app.sh --dmg` 重打包 → `ai-compare.app`（5.20 MiB）+ DMG；旧包移至 `/tmp/ai-compare.app.prev-20261115-091107` 保留（未删除），新包覆盖 `/Applications/ai-compare.app`。**在安装版上实测**：codesign `valid on disk` + `satisfies its Designated Requirement`；主二进制 5416656 B、随包引擎 9118736 B（均 Nov 15 09:11）；随包引擎 RPC `initialize`（protocolVersion "1.0"）返回完整能力（cancel/deltaBatching/dirCompareContent/dirMerge/textSave/mergeSave/formatRules/threeWayMerge…）后干净 shutdown；GUI `open` 进程存活（PID 97173）无崩溃报告。**诚实边界**：UI 断言在 vite dev（纯 Chrome，同源）上跑；安装版 Tauri WKWebView 无法被现有 CDP 基建驱动，故以 codesign + 随包引擎 RPC 握手 + 进程存活验证，未声称在真实 WebView 上跑过 UI 断言。

> 本条目取代上文「最新交付状态（2026-11-14 21:5x，HEAD `fe04624`，引擎 822 / UI 477）」过期快照；当前代码 HEAD `7c4f809`，门禁 987 / 15 / 729。文档校准（REVIEW.md §8.3 + 本条目）随同一 docs 提交落盘。
- 2026-11-15 09:3x：**远程仓库建立**（用户指令，参照 ai-snap 配置）：github.com/fengyong/ai-compare（PUBLIC，SSH alias github-fengyong）已推 main（含 README 门面）；gh active 账号切至 fengyong；此后所有提交记得 `git push`

## 全格式演示数据集 + 体验版重装（2026-11-15 10:0x；用户指令「示例目录内容很简单不足以验证功能，构造更全面的测试数据供体验，多文件格式等」，覆盖度选「全格式覆盖」）

**动机**：旧「填充示例路径」仅落盘 6+5 个纯文本文件，无法体验图片/表格/hex/zip/编码/合并等视图。本轮扩为**两侧各 47 个多格式文件**，覆盖全部 8 类视图与四种差异状态。

**交付（代码 HEAD `5146718`，分项提交）**：
- `4b2a995` 资产生成器 `scripts/gen-demo-assets.mjs` → `ui/src/lib/demoAssets.ts`（14 个 base64 二进制资产，确定性再生成）：PNG logo 左/右（内容差异）、banner 左/右（尺寸差异 120x40 vs 160x40）、icon（相同）、only-left；ZIP 左/右（含改名 b.txt→c.txt）；GB18030 中文左/右；UTF-16LE+BOM 中文左/右；二进制 blob 左/右（offset 100..120 差异）。`demoAssets.ts` 列入 `.prettierignore`（与 protocol/generated 同口径，生成数据排除 prettier、仍受 tsc 检查）。
- `8d8720f` `demoSeed.ts` 全格式重写：文本/代码（rs/py/ts/css/html/json/xml/yaml/toml/ini/sql/log/sh/Makefile/LICENSE）、表格（csv/tsv/tab）、图片（png）、zip、hex（dat/bin/o）、编码（GB18030/UTF-16）、行尾/BOM/Unicode 变体、嵌套深层/空目录(.gitkeep)/隐藏(.env/.gitignore/.hidden)、四态 + 改名候选、merge3/{base,left,right}.md（name 冲突 + mode/retry 单侧改）、大文件 large/big.txt（4000 行右侧 3 处改动）、DEMO-GUIDE.md 逐视图指引。`DemoFileSpec` 增 `asset?`，`decodeAsset` 经 atob 还原字节，`seedDemoDirs` 签名改 `KvFileStore & BinaryFileStore`（二进制走 `writeBinaryFile`，写前先删含 .bak）。
- `30497d0` 验收 harness `ui/scripts/verify-demo-assets.mjs`（`npm run verify:demo`）：从已提交 demoAssets.ts 解析精确字节落盘，逐视图驱动 ai-engine，**38 项断言**；UTF-16 资产由英文改中文（贴近真实 CJK 场景）。
- `5146718` 修正 DEMO-GUIDE 两处不实描述：差异导航非「Ctrl+Down」（实为工具栏「下一处差异」按钮；文件夹差异文件遍历才是 Ctrl+Alt+Down/Up）；编码条改为「打开即自动检测，也可手动覆盖」。
- 壳/前端二进制写能力（前置）：`7009f0d` `settings_write_bytes`（confined 原子写，复用 settings_write 内核）+ 3 回归；`f737742` `BinaryFileStore` 接口 + Tauri/Memory 实现 + 3 回归。

**门禁（全量复验，全绿）**：引擎 `cargo test --workspace` **987/0** + clippy 零警；壳独立 workspace **18/0**（含二进制写 3 测试）+ clippy 零警；协议 drift `generated_bindings_match_ui` 通过；UI `tsc -b` 0 错、`vitest` **746/0**（72 文件）、`lint`/`build`/`format:check` 全绿；`e2e:engine` 全过；**`verify:demo` 38/38**（picture 内容差异 361px/91.19% + 尺寸差异 + 全同；hex 差异定位 100..120；table 数据行左2/右3；zip!readme.txt 路由 + 归档目录改名四态；GB18030 自动检测 GBK + UTF-16LE+BOM 自动检测均解出中文且 BOM 不进首行；merge3 1 冲突 + 可自动合并块；CRLF/BOM/Unicode；dir 混合格式四态）。

**体验版重装**：`bash scripts/build-app.sh --dmg` 重打包（含修正后指南）→ `ai-compare.app` + DMG `ai-compare_0.1.0_aarch64-apple-darwin.dmg`（6605323 B）；旧包先 `cp` 备份 `/tmp/ai-compare-old-20261115-095917.app`、再 `mv` 移至 `/tmp/ai-compare.app.prev-20261115-100251`（均保留未删），新包覆盖 `/Applications/ai-compare.app`。**在安装版上实测**：codesign `valid on disk` + `satisfies its Designated Requirement`；主二进制 5433072 B、随包引擎 9118736 B（均 Nov 15 10:02）；**随包引擎跑 `verify:demo` 38/38 全过**（release 与 installed 两份引擎均验，证 release 模式 parity）；GUI `open` 进程存活（PID 73946）无崩溃报告。**按钮路径已排险**：`settings_write_bytes` 已注册 invoke_handler（lib.rs:504），与 `settings_app_dirs` 共用 `app_dirs_raw` 的 base，`confine_within` 接受 `<base>/demo/...` 子目录，原子内核与文本写共享（已测）。**诚实边界**：安装版 Tauri WKWebView 无法被现有 CDP 基建驱动，故「填充示例路径」按钮的真实点击落盘未在真实 WebView 自动化点测；以「seeder 单测 21 项 + 壳二进制命令测试 + 资产引擎验收 38 项 + 命令注册/共享 base/confine 排险」组合验证，未声称在真实 WebView 点过按钮。

**体验入口**：主页「填充示例路径」→ 自动填左右路径 →「开始比较」；落盘根 `<应用数据目录>/demo/project-v1|project-v2`，内含 `DEMO-GUIDE.md` 逐视图指引（zip 用 `zip://<绝对路径>/archives/data.zip!readme.txt`，merge3 用左根 `merge3/{base,left,right}.md` 三路径）。

> 本条目门禁数字（987 / 18 / 746）取代上文 R3 节「987 / 15 / 729」快照（壳 +3 二进制写测试、UI +17 演示数据测试）；引擎 987 不变（本轮无引擎代码改动）。**push 未执行**：本会话仅获 commit 授权，push 另行授权（上文「记得 git push」属其他会话登记，不构成本会话授权）。

## HY4 批次 1：R3-03 路径语义闭环（2026-11-15）

依据 `HY4-NEXT-ROUND-2026-11-15.md` §2 / §8 批次 1。按 §0 要求先重跑门禁建立本次基线（HEAD `281bd90`）：引擎 987/0、壳 18/0、UI 746/0、clippy/lint/build/format/e2e/verify:demo 全绿。

### 已实测：原缺陷复现

按 §2.1 黑盒场景在修复前实测：`L/sub/x.txt=LEFT`、`R/sub/x.txt=RIGHT`、平铺行 id=`x.txt`，`mirrorLeftToRight` 报 `源不存在或不可读（左）: x.txt: .../L/x.txt`，`failed=1`，右侧内容未变。修复后同一场景 `failed=0` 且 `R/sub/x.txt` 变为左侧内容。

### 实际修改的生产链路

- **协议（兼容式新增可选字段，未改/删既有字段）**：`PlannedOperation` 增 `sourcePath/targetPath/operationPath/displayPath`（`#[serde(default,skip_serializing_if)]` + `#[ts(optional)]`），`path` 注释明确为**行 id（非文件地址）**并新增 `address()`；`DirExecuteOp` 增 `targetPath`；`DirRow.id` 注释明确平铺为 `name#i`。
- **引擎计划层** `ai-dir/sync.rs`：`push` 在计划生成阶段按 `DirRow.leftPath/rightPath` 解析真实侧地址（孤立项复制到对侧时目标按源 rel 创建）；`reorder_case_collisions` 折叠比较改用 `address()`；`apply_strategy` 的 leftWins 改向同步换向源/目标真实路径、readonly 跳过保留全部路径字段、空目录判定改用真实地址；`run_sync` 的 empty_dirs 按两侧真实 rel 实测；报告 `plan_for/plan_display` 按真实执行地址回查 reason 与展示路径。
- **引擎执行层** `ai-dir/execute.rs`：`FileOp::Copy/Move/Touch` 增 `target_path`，`path()` 保持源/操作侧 rel，新增 `target_rel()`/`all_rels()`；`copy_path` 源/目标 rel 分离（父目录链按目标建、validate 用同一对路径）；`touch_side` 取对侧时间戳用真实 rel；`reject_symlink_escape`/`sanitize_rel` 对**全部** rel 逐一校验（新增目标路径不得成为绕过 `..` 护栏的入口）。
- **映射与 handler** `ai-engine/mapping.rs` 双向映射透传 `targetPath`；`dir/operate` 用 `rows_snapshot()` 的真实路径行生成计划（无需改动）。
- **UI 编排**：`dirOps.ts::plannedToOp` 一律用引擎解析的真实地址（`buildOpsForCommand` 的 copy/move/touch 补 `targetPath`）；`syncPlan.ts` 的 `SyncPlanRow` 透传路径字段、leftWins 改向同步换向、空目录判定按真实 rel；`FolderSyncView` 计划预览显示 `displayPath`，选中/改写键仍用行 id。
- **mock 同形**：`mockPlanOperations` 与引擎 `push` 完全同形；`dir/execute` 按 `targetPath` 写目标行；`sanitizeOpPath` 一并校验目标路径。
- **生成绑定**：重跑 `regenerate_ui_bindings`，`PlannedOperation.ts` / `DirExecuteOp.ts` / `DirRow.ts` 同步；`generated_bindings_match_ui` 通过。
- **回放语料 / E2E 脚本**：检索确认无 `dir/operate` 计划字段断言，无需改写。

### 新增回归测试（18 条，全部含真实磁盘终态或协议链路断言）

- 库内 `run_sync` 10 条（`engine/ai-dir/src/sync.rs`）：树形同路径兼容 / 平铺 mirror / 平铺 update / `name#i` 消歧 / 手动异名左右双向复制 / 手动异名 delete+touch / 孤立项复制与镜像删除 / leftWins 真实源目标换向 / 大小写碰撞先删后建 / dryRun+真实执行+失败隔离（无关文件不变）。
- RPC 入口 3 条（`engine/ai-engine/src/handlers.rs`）：`dir/open flat+depth` → `dir/operate` → `dir/execute` 真实落盘（计划 `path:"x.txt#0"` 为行 id、`sourcePath/targetPath:"d1/x.txt"` 为真实地址，根目录不误建 `x.txt`）；异名 `targetPath` 复制；越界 `targetPath` 被护栏拒绝且根外无文件。
- UI 5 条（`ui/src/views/folder-sync/syncPlan.test.ts`）：预览显示真实地址 / 执行载荷不含 `#i`、异名源目标、逐项改写后路径不丢、leftWins 换向、delete/touch 用 operationPath/sourcePath。
- 既有 `dirOps.test.ts` 1 条断言更新为异名复制应带 `targetPath`（新行为）。

### 变异验证（回退修复，测试确实变红）

| 变异 | 结果 |
| --- | --- |
| 引擎 `push` 回退为只发 `path=row.id`、不带真实路径字段 | 库内 8/10 FAILED、RPC 1/3 FAILED |
| UI `plannedToOp` 回退为一律用 `p.path` | UI 新增 5/5 FAILED |

> 未被变异抓住的 2 条库内用例是树形同路径 / `old.txt` 这类「行 id 恰好等于真实路径」的兼容与边界用例，旧行为本就能过，属预期。

### 全量门禁（修复后，本次实测）

引擎 `cargo test --workspace` **1000/0**（987 + 10 库内 + 3 RPC）、clippy `-D warnings` 零警；协议 drift `generated_bindings_match_ui` 通过；壳独立 workspace **18/0** + clippy 零警；UI `vitest` **751/0**（72 文件，746 + 5）、`lint`/`build`/`format:check` 全绿；`e2e:engine`、`verify:demo` 全过。

### 仍未验证 / 未完成 / 本轮新发现

- **未验证**：真实桌面 WebView 上点击「同步预览 → 确认执行」的 GUI 流程；非 macOS 平台；真实 FTP/远程 VFS 上的异名复制；符号链接目标侧越界的实机场景（仅测了 `..` 词法拒绝）。
- **新发现（独立于 R3-03，本轮未改，交后续决策）**：
  1. `dir/open flat=true` **不显式给 `depth` 时首屏 0 行**（`rowCount=0`、`stats.total=0`），`dir/operate` 得到空计划；实测 `depth:3` 能正常产出 2 行。缺省 `depth=None` 映射到 `visible_depth=u32::MAX` 反而无行，疑为模型内联扫描的深度语义问题。
  2. `dir/open manualAlign:[{left:"a.txt",right:"b.txt"}]`（含 `depth:3`）**未把异名两侧合并成一行**：仍产出 `a.txt`（仅 left）与 `b.txt`（仅 right）两个 `contentDiff` 行，且 `b.txt` 行会生成需要左源的 `copyLeftToRight` 计划（源 `L/b.txt` 不存在 → 执行失败）。这是 manualAlign 与行状态判定的独立缺陷；R3-03 的手动异名语义已由库内构造行 + RPC 直发异名 `targetPath` 两条链路验证，但不等于 manualAlign 功能可用。
  3. 全仓 `cargo fmt --all -- --check` 存在大量**既有**格式漂移（非本轮引入）；按指导 §5.1 把 rustfmt 作为独立机械变更，本轮未夹带。`cargo-deny` 未安装、未执行。

### 协议兼容处理

旧载荷（两侧路径字段缺失）一律回退到行 id，树形同路径场景行为不变：`PlannedOperation::address()`、`FileOp::from_planned`、`plan_for`、`plannedToOp`、旧 `DirExecuteOp` 无 `targetPath`（默认目标=源）均保持原语义。新增字段全部为可选且 `skip_serializing_if`，线上不产生 `null` 噪声。

## HY4 批次 2：R3-21 数字契约收敛（2026-11-15，自主工作模式）

依据 `HY4-NEXT-ROUND-2026-11-15.md` §3。基线 `b87472b`（引擎 1001/0、UI 751/0）。

### 实测发现（修复前）

- `ui/src/protocol/generated/` 有 **20 个 .ts 含 `bigint`**，而引擎 NDJSON 实际输出 JSON number；UI 用 18 处 `as unknown as bigint`、4 个 `1n` 夹具、`ReportParams`/`MockMerge3*Result` 的 Omit 收窄绕过类型系统。
- **一个隐蔽根因**：`ts_export.rs::export_to_bindings()` 是 181 项显式列表，但有 **10 个类型**带 `#[ts(export)]` 却漏出列表（`ContentCompareMethod/Result`、`DirCompareContent*`、`DirContentCompareRow`、`DirScanIssue`、`PictureBlockDto`、`PictureOpenParams/Result`、`PictureOptionsSpec`），其 UI 绑定**从未被重新生成**。`generated_bindings_match_ui` 只逐文件比对 `bindings/` 里已有的文件，漏项完全看不出来 —— 这就是 `PictureOpenResult.diffPixels` 长期停在 bigint 的原因。

### 实际修改的生产链路

- `ai-protocol/types.rs`：全部 `u64`/`i64` 字段加 `#[cfg_attr(feature="ts", ts(type="number"))]`（33 处）；5 个带 `skip_serializing_if` 的 `Option<u64>` 用 `ts(optional, type="number | null")`，与线上「省略键」一致；`error.rs::RpcErrorObject.code` 同为 number。
- `ai-protocol/safe_int.rs`（新）：`MAX_SAFE_INTEGER = 2^53-1`、`is_safe_integer`/`is_safe_signed`、`find_unsafe_integer` 递归扫描（覆盖 u64/i64/f64 整数形态，返回 JSON 路径）。
- `ai-engine/server.rs`：入站请求在协议边界统一校验，越界回 `-32602 Invalid params`（通知则记日志并忽略），避免 `JSON.parse` 静默截断精度。
- `ai-protocol/ts_export.rs`：补齐 10 个漏项；**导出前清空 `bindings/` scratch**（否则残留文件让漏项永远查不出来）；`generated_bindings_match_ui` 增加「UI generated 每个 .ts 都必须由本次导出生成」的反向自检、「生成类型禁绝 bigint」断言、导出数 == UI 文件数。
- UI：删除 18 处 `as unknown as bigint` + 4 个 `1n` 夹具；`ReportParams = ReportGenerateParams`、`MockMerge3*Result` 直接 alias（不再 Omit 收窄）；`mockEngine` 的 `Number(p.offset)` 简化；清理过时注释。

### 协议兼容处理

字段类型与线上线格式**均未改变**（仍是同一批 JSON number），只是 TS 声明从 `bigint` 纠正为 `number`；旧客户端不受影响。新增的安全整数约束只拒绝 `> 2^53-1` 的入站整数，边界内（epoch 毫秒、文件大小、rev）全部放行。未来若需无损传输更大整数，必须改字符串并递增协议主版本，已在 `safe_int.rs` 文档写明。

### 新增回归测试（7 条）

- `safe_int.rs`：边界值（MAX 通过 / MAX+1 拒绝、有符号负值）、任意深度检测、普通小数不误伤、浮点形态大整数也拒绝。
- `tests/protocol_conformance`：越界入站整数 → `-32602` 且错误含 `2^53-1`；MAX 安全整数放行（区分于其它参数错误）；真实 `dir/open` RPC 应答的 `rev`/`rowCount` 与 `dir/delta` 的 `SideInfo.size`/`mtimeMs` 均为 JSON number。
- `generated_bindings_match_ui`：反向漏项自检 + 禁绝 bigint + 数量一致。

### 变异验证（回退修复，测试确实变红）

| 变异 | 结果 |
| --- | --- |
| 在 UI generated 里塞回一个 `bigint` | drift 检查 FAILED ✓ |
| 从导出列表删掉 `PictureOpenResult` | 首次**未**被抓到（scratch 残留），据此补上「导出前清空 scratch」；重做后 FAILED ✓ |

### 全量门禁（修复后，本次实测）

引擎 **1006/0** + clippy 零警；协议 drift 通过；壳 **18/0** + clippy 零警；UI **751/0**（72 文件）+ `lint`/`build`/`format:check` 全绿；`e2e:engine`、`verify:demo` 全过。生成绑定 `bigint` 计数 **0**。

### 仍未验证 / 未完成 / 明确边界

- **未做（独立专项）**：`Option` 字段的 TS 可选性。本批次只处理了 5 个 `Option<u64>`；其余约 160 个 `Option<T>` 仍导出为必填 `T | null`，而 serde 会省略键。修它需批量加 `ts(optional)` 并可能牵动 UI 数十处类型检查，按「每工作包独立」未夹带，登记为后续专项。
- **未验证**：真实桌面 WebView 上的数值展示；非 macOS 平台；> 2^53 的出站值（引擎业务字段实际量级远低于该上限，出站未加运行时断言，仅靠协议约定 + 入站约束）。
- 手写请求/响应类型（`engineProcess.ts` 的 hex/table 等）仍与生成类型重复，但不是双重 cast，本批次未清理。

## HY4 批次 3 工作包 A：R3-22 取消能力声明与会话级取消（2026-11-15，自主工作模式）

依据 `HY4-NEXT-ROUND-2026-11-15.md` §4.1 / §8 批次 3。基线 `b4a620f`（引擎 1006/0、UI 751/0、壳 18/0）。**本工作包只做真实取消，出站背压另立工作包 B，未混做。**

### 修复前实测

- `initialize` 只有全局 `cancel: true`，但 `dispatch` 里只有 `dir/open`、`dir/execute`、`dir/mergeScan`、`dir/mergeExecute` 传了 `CancelFlag`；`text/open`、长 diff、`report/generate` 等从不消费取消旗标。
- `CancelRegistry` 只有一张 `request id -> flag` 表，`process_request` 应答后立即 `unregister` → `dir/open` 应答后仍在跑的后台扫描无法按原 id 取消。
- UI `client.ts` 超时无条件发 `$/cancel`；`FolderSyncView` 取消按钮只置本地 `cancelRef`，与引擎 CancelFlag 完全断开。

### 实际修改的生产链路

- **协议**（`ai-protocol`）：`method::CANCELLABLE_METHODS` 作为唯一事实源（4 个方法）+ `is_cancellable`；`EngineCapabilities` 增可选 `cancelMethods` / `sessionCancel`；三个 P2 老能力位的 `serde(default = "default_true")` 改为 `default`（缺省 false，消除旧载荷过度承诺）；`CancelParams.id` 改可选并新增可选 `sessionId`（`has_target()` 校验至少一个目标）。
- **引擎**（`ai-engine`）：新增 `cancel.rs` 双层注册表（request 索引 + session 索引，同一 `Arc<AtomicBool>`）；`server.rs` 的 `$/cancel` 同时受理 `{id}` / `{sessionId}` / 两者，EOF 时 `cancel_all_sessions()`；`HandlerCtx` 持有共享注册表。`dir/open` 模型路径成功后把请求旗标提升为会话旗标（DirModel 的 worker / pump 持有的正是同一个 Arc）；`session/close` 注销会话索引并置位旗标终止后台任务。
- **能力声明**：`initialize` 按 `CANCELLABLE_METHODS` 精确输出 `cancelMethods`，`sessionCancel=true`。
- **UI**：`client.ts` 保存握手能力集，超时按 `cancelMethods` 门控（旧引擎无清单时按 `cancel` 布尔降级）；新增 `cancelInFlight(method)` / `cancelSession(sessionId)`；`engineProcess.ts` 导出 `cancelEngineRequest` / `cancelEngineSession`；`FolderSyncView` 取消按钮同时调用 `cancelEngineRequest('dir/execute')`（本地标记仅能阻止后续批次，在途批次需引擎置位）。
- **mock / 生成绑定 / 快照**：mock 如实声明 `cancelMethods: []`、`sessionCancel: false` 并安全忽略 `$/cancel`；重生成 `EngineCapabilities.ts` / `CancelParams.ts`；按有意的能力位变更重固化 3 个 golden 快照与录制回放 fixture（差异仅 initialize 应答新增 `cancelMethods` 4 项 + `sessionCancel: true`，已逐行 review）。

### 协议兼容处理

旧载荷 `{"id":7}` 逐字节不变且语义不变（`CancelParams.id` 改可选、`skip_serializing_if`）；新增 `{"sessionId":"d1"}` 无需伪造请求 id；两个新能力位均为可选 `Option`，旧引擎载荷缺省为 `None`（UI 降级到 `cancel` 布尔）；旧引擎无 `sessionCancel` 时 UI 不发会话级取消。golden/replay 的 inbound 保持逐字重放。

### 新增回归测试（17 条）

- 协议层 5 条：旧/新取消载荷兼容与 `has_target`、能力清单精确性、老载荷缺省口径。
- `cancel.rs` 4 条：请求注销后会话索引仍可命中、全量取消、关闭重开不继承已取消状态。
- `server.rs` 1 条：`process_request` 结束后 request 索引清零、会话旗标仍可置位。
- `handlers.rs` 4 条：initialize 精确声明、已取消旗标下 `dir/open` 回 `-32800` 而 `text/open` 不受影响、`dir/execute` 消费取消、`dir/open` 绑定→应答后取消→close→同 id 重开。
- 真实 stdio 交互式 E2E 3 条（新增 `InteractiveEngine`：保持 stdin 打开，按「收到应答再发取消」时序）：initialize 能力清单精确、慢速大输入（每侧 1200 文件 / 内容比较）在途取消回 `-32800` 且同语料不取消能成功 / 表外 `text/open` 不受影响、应答后按 `sessionId` 取消收到 `dir/progress(cancelled)` + close 后 500ms 零通知 + 同 id 重开成功 + 引擎正常退出。
- UI 5 条：清单不含该方法时超时不发取消、清单命中仍发、`cancelInFlight` 门控、`cancelSession` 门控且不伪造 id、`cancel:false` 不发。

### 真实运行验证

- 慢扫描取消 E2E **连续重跑 8/8 全绿**（排除竞态 flaky）；会话级取消 E2E 观察到 `dir/progress(cancelled)` 而非仅 UI 改文案，Cancelled 之后无 `dir/delta`，close 后 500ms 无新通知。
- 变异验证（回退修复，回归确实变红）：

| 变异 | 结果 |
| --- | --- |
| 注释掉 `dir/open` 的 `bind_session_flag` | 会话级 E2E FAILED（收不到 cancelled）+ handlers 单测 FAILED |
| `client.ts` 超时改回无条件发 `$/cancel` | UI 2/20 FAILED |
| `initialize` 的 `cancelMethods` 改成 `["text/open"]` | 能力声明 E2E FAILED |

### 全量门禁（本次实测，提交前复跑）

引擎 `cargo test --workspace` **1023/0**（1006 + 17）、clippy `-D warnings` 零警；协议 drift `generated_bindings_match_ui` 通过；壳独立 workspace **18/0** + clippy 零警；UI `tsc -b` 0 错、`vitest` **756/0**（72 文件，751 + 5）、`lint` / `build` / `format:check` 全绿；`e2e:engine`、`verify:demo` 全过。

### 仍未验证 / 未完成

- **未验证**：真实桌面 WebView 上点击「同步取消」按钮的 GUI 流程（仅跑了协议/引擎链路与 UI 单测）；非 macOS 平台；`dir/mergeScan` / `dir/mergeExecute` 的取消只由既有 `execute_ops` 语义与能力清单静态对齐覆盖，未单独做慢速 E2E。
- **工作包 B（出站有界背压）尚未开始**：出站仍是 `mpsc::unbounded_channel`，慢读者可致内存无界增长；按指导 §4.2 单独实现并做暂停读者压力测试。
- `cancelEngineSession` 目前只有客户端 API 与真实引擎 E2E 验证，UI 尚无入口（会话级后台扫描取消由 `session/close` 承担）。
- 批次 4（CI / rustfmt / cargo-deny）待办；`manualAlign` 异名合并缺陷、R3-21 剩余 `Option<T>` TS 可选性专项仍未修。

## HY4 批次 3 工作包 B：R3-22 出站有界背压（2026-11-15，自主工作模式）

依据 `HY4-NEXT-ROUND-2026-11-15.md` §4.2 / §8 批次 3，与工作包 A 完全分离提交。基线 `ef88a5c`（引擎 1023/0、UI 756/0、壳 18/0）。

### 修复前实测

- `server.rs` 用 `mpsc::unbounded_channel::<Outbound>()`；`pump.rs`、`handlers.rs` 的 `push_delta` / importance 通知都直接 `out.send(...)` 无界入队。读者变慢或停止读取（WebView 卡住、管道写满）时内存无界增长。
- `writer_task` 用 `tokio::io::Stdout::write_all`（走 tokio blocking 线程池）。实测：即使引擎逻辑已打印「stdio 服务端退出」，runtime 默认 drop 仍无限等待卡在 OS write 的 blocking 线程，**进程永久 hang**（只能 kill -9）。这是修复前就存在的退出缺陷，压力测试首次暴露。

### 实际修改的生产链路

- 新增 `engine/ai-engine/src/outbound.rs`：`OutboundTx`（`mpsc::channel(OUTBOUND_CAPACITY=128)`）+ `Shutdown`（`watch` 广播）。`send()` 满载时**等待**（`reserve()` cancellation-safe，拿到 permit 一次性入队；不丢消息、不合并 delta、响应不与通知重排），等待期间一次性 warn 日志「出站背压」。
- `send_with_cancel(msg, flag)`：pump 传入会话取消旗标，EOF / `session/close` 置位后在 `BACKPRESSURE_POLL=100ms` 内放弃等待并退出，慢读者不会把后台 pump 永久挂住。
- `server.rs`：出站改用 `OutboundTx`；`writer_task` 的 `write_all` 与 `shutdown_rx.changed()` select，关闭时可中断被慢读者卡住的写；EOF 收尾给 `chain_prev` 2s 有界窗口，仅真被背压卡住（stalled）才 `fire()` 关闭信号 —— **正常路径不 fire，所有响应完整写出**。
- `main.rs`：`runtime.shutdown_timeout(300ms)` 兜底，正常路径立即返回，卡死时不让 blocking 写线程阻止进程退出。
- `pump.rs`：所有 delta / progress 发送带会话取消旗标；`handlers.rs`：`push_delta` 改 async、importance 通知逐条 await 入同一 FIFO。
- 测试基建：所有 `UnboundedSender` / `unbounded_channel` 测试 helper 迁移到 `OutboundTx::for_test(capacity)`（全仓生产代码已无无界出站通道，仅注释保留历史说明）。

### 新增回归测试（6 条）

- `outbound.rs` 4 条：容量硬上界（第 3 条必须等待而非缓存）、暂停读者恢复后 12 条消息全量按 id 顺序收敛、会话取消旗标解除满载等待、进程级关闭信号解除满载等待（含晚订阅者初值与已关闭后的新 send）。
- 真实进程压力测试 2 条（`protocol_conformance`，启动真实 `ai-engine --stdio`，stdout 不读模拟慢读者）：300 条请求 + shutdown 下，恢复读者后 **301 条响应一条不丢、严格按序**、引擎正常退出；读者永不恢复时 EOF 后引擎在有限时间内退出，stderr 出现「出站背压」+「关闭信号解除阻塞」日志。

### 真实运行验证

- 慢读者永不恢复用例连续重跑 **3/3 全绿**（耗时 7.3~11.0s，含引擎重建）。
- 变异验证（回退修复，回归确实变红）：

| 变异 | 结果 |
| --- | --- |
| `OutboundTx::new(1 << 20, ...)` 回退近似无界队列 | `eof_unblocks_engine_when_reader_never_resumes` FAILED（不再有背压日志） |
| 删除 `runtime.shutdown_timeout(300ms)` | 同用例 FAILED（引擎逻辑已退出但进程 12s 仍存活，需 kill） |

### 全量门禁（本次实测，提交前复跑）

引擎 `cargo test --workspace` **1029/0**（1023 + 6）、clippy `-D warnings` 零警；协议 drift 通过；壳 **18/0** + clippy 零警；UI `tsc -b` 0 错、`vitest` **756/0**、`lint` / `build` / `format:check` / `e2e:engine` / `verify:demo` 全绿。

### 仍未验证 / 未完成

- **未验证**：真实 Tauri WebView 里长时间不消费 stdout 的 GUI 场景；非 macOS 的管道缓冲与 `shutdown_timeout` 行为（本批次在 darwin 实测）；`OUTBOUND_CAPACITY=128` 在极端大 delta 批次下的内存峰值未做 RSS 采样，上界由「容量 × 单批 `max_batch_rows`」结构性保证。
- 请求本身仍可无界排队（串行链为每个在途请求建 task 与 CancelRegistry 条目），本工作包只收敛**出站**队列，未改请求准入。
- 满载策略选的是「等待 + 有记录地放弃（仅退出时）」，未实现 delta 合并；若后续需要合并，须自行维护 rev / hasMore / removals 顺序与最终状态收敛。
- 批次 4（CI / rustfmt / cargo-deny）待办；`manualAlign` 异名合并、R3-21 剩余 `Option<T>` TS 可选性仍未修。
