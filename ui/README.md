# ai-compare UI（Tauri 2 + React 18 + TypeScript）

ai-compare 的前端与桌面壳。架构对齐 `docs/03-架构设计文档.md`（UI 层 = M30 应用壳 / M31 视图插件 / M32 对齐文本视图 / M33 虚拟列表 / M34 通用服务），当前为 **P0 Walking Skeleton** 交付（见 `docs/04-开发计划.md` §3）。

## 启动方式

### 1. 纯前端开发（浏览器演示模式，无需引擎）

```bash
cd ui
pnpm install
pnpm dev          # http://localhost:5173
```

浏览器内没有 Tauri 环境，UI 自动降级为**内置演示引擎**（纯 TS 实现的协议子集，见 `src/ipc/mockEngine.ts`）：合成目录树渐进扫描着色、双击文件打开文本 diff，完整走通端到端闭环。

### 2. 桌面应用（依赖引擎 sidecar）

```bash
# 前置：先构建引擎二进制
cargo build -p ai-engine          # 产物：<仓库根>/target/debug/ai-engine（workspace 根 target）

cd ui
pnpm tauri dev                    # Tauri 壳 spawn ai-engine --stdio，NDJSON JSON-RPC
```

引擎二进制查找顺序（见 `src-tauri/src/lib.rs`）：

1. 环境变量 `AI_ENGINE_PATH`（或 dev 模式传入参数）；
2. dev 约定路径 `<仓库根>/target/debug/ai-engine`；
3. release：与 UI 可执行文件同目录。

引擎缺失时应用会显示降级横幅：可重试，或一键切换内置演示引擎（数据为模拟，非真实文件系统）。

### 其他命令

```bash
pnpm build        # 类型检查 + 产物构建（tsc -b && vite build）
pnpm test         # vitest 单测（协议客户端 / 滚动映射 / 行内 diff / NDJSON 重组）
pnpm lint         # ESLint
pnpm format       # Prettier
pnpm tauri build  # 桌面安装包（需要 Rust 工具链）
```

## 目录结构（docs/03 §3.4 / §7 命名）

```
ui/
├── src/
│   ├── shell/            # M30 应用壳：标签栏 / 视图路由 / zustand store / 状态栏
│   ├── views/            # M31 视图
│   │   ├── home/             # 主页占位（双路径 + 开始比较）
│   │   ├── folder-compare/   # 目录比较：虚拟滚动表 + 窗口取数缓存 + 状态着色 + 统计
│   │   └── text-compare/     # 文本比较：CM6 对齐层 / 行级+行内着色 / 同步滚动 / 导航
│   ├── components/       # M32/M33 共享组件（随视图增长迁入）
│   ├── protocol/         # 协议层：types.ts（手写，将来以 generated/ 为准）+ JSON-RPC 客户端
│   │   └── generated/        # 引擎线 ts-rs 生成类型的目标目录（占位）
│   ├── ipc/              # Tauri stdio 透传 + 内置演示引擎 + 连接管理（优雅降级）
│   ├── lib/              # 纯函数工具（行内 diff 核心 / 环境探测 / 格式化）
│   └── theme/            # 色板集中管理（palette.ts，BC 颜色语义）+ 全局样式
├── src-tauri/            # Tauri 2 Rust 壳：窗口 + ai-engine spawn/stdio 透传
└── package.json
```

## 协议（NDJSON JSON-RPC over stdio）

- 消息模型见 `src/protocol/types.ts`（对齐 docs/03 §2.3 / §4.1 / §4.2）；引擎线会以 ts-rs 生成同构类型到 `src/protocol/generated/`，**届时以生成类型为准**。
- 客户端 `src/protocol/client.ts`：initialize 握手、超时 + `$/cancel`、通知分发、写入串行化；传输层 `src/ipc/tauriTransport.ts`（Tauri command/事件透传，Tauri 仅作字节管道，ADR-003）。
- P0 方法集（与真实引擎 handlers.rs 对齐）：`initialize`、`dir/open`、`dir/delta`(通知)、`text/open`、`text/sections`、`text/lines`(mock 扩展，引擎 P1)、`session/close`、`shutdown`。

### 3. 真实引擎端到端验收（M1 闭环门禁）

```bash
cargo build -p ai-engine          # 仓库根执行；或 AI_ENGINE_PATH 指定二进制
cd ui && pnpm e2e:engine          # 退出码 0=通过（含 tempdir 场景与状态断言）
```

## 约定

- 界面语言中文优先，字符串集中在 `src/strings.ts`（为 i18n 留口）。
- 暗色主题优先，颜色 token 集中在 `src/theme/palette.ts`（语义对齐 docs/01 附录 C：黑=相同、灰=未知/较旧、紫=孤立、红=不同/较新、蓝=不重要、青绿=被过滤、黄=未扫描）。
- 状态管理：React 内置 + zustand（仅标签/会话等轻量 UI 状态）；大数据（目录行窗口、文本行）在视图级非响应式缓存。

## P0 已知边界（后续阶段交付）

- 目录树展开/排序/过滤/文件操作/选择模型：P2/P6；当前为引擎排好序的平铺列表。
- 文本编辑、概览条、查找替换、编码切换、语法高亮：P5。
- `text/inlineDiff` 引擎侧 P0 未开放（能力位上报 false，UI 以客户端词级 diff 兜底）：P1 开放后切引擎懒计算。
- 断线重连/resync、协议录制回放（引擎 `--record` 已具备）：P4/P5 接入。
- 主页类型大按钮/会话树/设置界面/命令面板：P4。
