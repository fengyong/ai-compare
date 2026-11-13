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
- [ ] **M1 Walking Skeleton（进行中，对应 docs/04 P0 阶段）**：端到端最小闭环（选两个本地文件夹 → 目录比较 → 双击文本文件 → 行级+行内 diff → 展示）
  - [x] UI 线：Tauri 2 + React 壳 + 目录视图 + 文本视图（CM6）+ 双击开子会话 ✅（44/44 测试、build 零错误、浏览器端到端冒烟通过，含差异回传着色；提交 1297af9）
  - [ ] 引擎线：Rust workspace（ai-protocol/ai-core/ai-vfs/ai-dir/ai-engine）+ aic CLI + golden 测试（agent 运行中）
  - [ ] 集成验收：引擎+UI 真实对接（UI 当前用内置演示引擎 mock，transport 已就绪；引擎完成后切换）、`aic diff` 与 `diff -u` 对拍、PB-04 冒烟基准
- [ ] M2 MVP（P0 需求）
- [ ] M3 V1.0（P0+P1）
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
| 2026-11-13 21:3x | Agent「P0 引擎线」→ Rust workspace + 引擎 crates + aic CLI + 测试 | 运行中（22:24 巡检活跃） |
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
