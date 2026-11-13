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
  - [ ] 引擎线：Rust workspace（ai-protocol/ai-core/ai-vfs/ai-dir/ai-engine）+ aic CLI + golden 测试（agent 运行中）
  - [ ] UI 线：Tauri 2 + React 壳 + 目录视图 + 文本视图（CM6）+ 双击开子会话（agent 运行中）
  - [ ] 集成验收：引擎+UI 对接、`aic diff` 与 `diff -u` 对拍、PB-04 冒烟基准
- [ ] M2 MVP（P0 需求）
- [ ] M3 V1.0（P0+P1）
- [ ] M4 V2.0（含 Pro 级功能）

## 当前阶段与断点

**当前**：等待 M0.3 的架构设计 agent 完成（后台运行中）。

**下一步（M0.3 完成后立即执行）**：
1. 验收 docs/03、docs/04（行数、章节完整性）
2. 初始化 monorepo（git init、目录结构按 docs/03 的规划：engine/、cli/、ui/、docs/、tests/）
3. 按 docs/04 的「首个可运行切片」启动开发：优先 Rust diff 引擎核心（行级 Myers/patience + 行内词级 diff + diff3）+ 目录扫描对齐，配套单测；UI 壳随后
4. 开发方式：后台 agent 并行（引擎线 / 测试线 / UI 线），每个 agent 完成后由主会话验收并更新本文件
5. 23:00 后加大并行度（多 agent 同时开发不同模块）

## 运行中任务登记

| 时间 | 任务 | 状态 |
| --- | --- | --- |
| 2026-11-13 21:1x | Agent「架构设计与开发计划」→ docs/03 + docs/04 | ✅ 完成 |
| 2026-11-13 21:3x | Agent「P0 引擎线」→ Rust workspace + 引擎 crates + aic CLI + 测试 | 运行中 |
| 2026-11-13 21:3x | Agent「P0 UI 线」→ Tauri2+React 壳 + 目录/文本视图 | 运行中 |

（agent 完成或中断时更新此表；看护任务据此判断是否有工作进行中）

## 开发约定

- 每完成一个模块/阶段：更新本文件 + git commit（信息格式：`M<里程碑>-<模块>: 摘要`）
- **开发 agent 不自行 git commit、不编辑 PROGRESS.md**（由主会话统一提交与登记，避免并发冲突）
- 代码须带单测；引擎层（Rust）保持无 UI 依赖，可全量单测
- 遵循 docs/03 架构决策；如需偏离，先在 docs/03 追加 ADR 再动手
- 历史决策详见 docs/01、docs/02
- 工具链：node v26.8.1 / pnpm 12.18.0 已装；rustup stable 安装中（完成后 cargo 在 `$HOME/.cargo/bin`，agent 执行前先 `source "$HOME/.cargo/env"`）

## 事件日志

- 2026-11-13 21:0x：目录确认为空，启动双调研 agent
- 2026-11-13 21:1x：调研完成（docs/01、docs/02 验收通过）；启动架构设计 agent；建立本进度文件与看护定时任务（每 20 分钟）
- 2026-11-13 21:2x：git init（main 分支）+ 首次提交（docs/01、docs/02、PROGRESS.md）；等待架构 agent 完成
