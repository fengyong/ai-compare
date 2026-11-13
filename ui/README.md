# ai-compare UI（Tauri 2 + React 18 + TypeScript）

ai-compare 的前端与桌面壳。架构对齐 `docs/03-架构设计文档.md`（UI 层 = M30 应用壳 / M31 视图插件 / M32 对齐文本视图 / M33 虚拟列表 / M34 通用服务），当前交付 **P0 Walking Skeleton + P4 UI 壳与会话体系**（见 `docs/04-开发计划.md` §3）。

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
│   ├── shell/            # M30 应用壳：多窗口/标签栏 / 视图路由 / zustand slice / 状态栏
│   │   ├── store.ts          # slice 组合（windows/tabs/sessions/ui，见 shellState.ts）
│   │   ├── viewRegistry.ts   # M31 视图插件注册表 + ViewCtx 注入
│   │   ├── commands.ts       # 命令注册表（菜单/工具栏/快捷键统一来源）
│   │   ├── shortcuts.ts      # 键位解析/匹配/冲突检测（默认 BC 键位表附录 G）
│   │   ├── shortcutEngine.ts # 全局 keydown → 命令引擎（重绑定 + 冲突确认）
│   │   ├── ContextMenu.tsx / Dialogs.tsx  # 右键菜单 / 关闭确认·另存·重命名对话框
│   │   └── settings/         # 设置对话框：分页导航 + schema 驱动表单 + 作用域联动
│   ├── views/            # M31 视图
│   │   ├── home/             # 主页：六类类型大按钮 / 会话树面板 / 最近列表 / 拖放启动
│   │   ├── folder-compare/   # 目录比较：虚拟滚动表 + 窗口取数缓存 + 状态着色 + 统计
│   │   └── text-compare/     # 文本比较：CM6 对齐层 / 行级+行内着色 / 同步滚动 / 导航
│   ├── session/          # M06 会话层（UI 侧）：模型/作用域合并/存取层/会话服务
│   │   ├── model.ts          # sessions.json 等 4 文档的类型化模型 + 容错解析
│   │   ├── scopes.ts         # 四层作用域合并（global→kind→parent→session）+ 写回
│   │   ├── storage.ts        # 原子写计划/.bak 回滚/MRU 纯函数（单测口径）
│   │   ├── storageAdapter.ts # IO 后端：memory / browser(localStorage) / tauri(原子写)
│   │   └── service.ts        # SessionService：CRUD/锁定/MRU/作用域写回/恢复报告
│   ├── protocol/         # 协议层：types.ts（UI 本地辅助）+ JSON-RPC 客户端
│   │   └── generated/        # 引擎线 ts-rs 生成类型（P2 已扩展判据/过滤器/属性）
│   ├── ipc/              # Tauri stdio 透传 + 内置演示引擎 + 连接管理
│   │   └── reconnect.ts      # 断线重连状态机（崩溃→自动重启一次→resync/全量重放）
│   ├── lib/              # 纯函数工具（行内 diff 核心 / 环境探测 / 格式化）
│   └── theme/            # 色板集中管理（palette.ts，BC 颜色语义）+ 全局样式
├── src-tauri/            # Tauri 2 Rust 壳：窗口 + 引擎 spawn/stdio + 设置文件原子写
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
- 状态管理：React 内置 + zustand slice（窗口/标签/会话等轻量 UI 状态）；大数据（目录行窗口、文本行）在视图级非响应式缓存。
- 会话/设置持久化（SET-001）：`sessions.json` / `session-defaults.json` / `settings.json` / `state.json` 四文档；桌面存平台设置目录（可执行文件旁 `portable.ini` → 便携模式存程序目录），浏览器 dev 存 localStorage（键 `aic:<文件名>`）；原子写（tmp+rename）+ `.bak` 回滚 + 损坏自动恢复提示，语义以 `src/session/storage.ts` 纯函数为口径（Rust 侧同语义实现）。

## P4 已知边界（后续阶段交付）

- 原生多窗口（Tauri WebviewWindow + 跨窗口状态同步）：P13；当前"拖出成新窗口/合并窗口"为应用内窗口帧（store 级多窗口完整，浏览器可演示）。
- 工作区（另存/加载窗口布局，FR-WIN-003）：P13。
- 视图专属命令（复制到对侧 Ctrl+L/Ctrl+R、空格标记等）已注册占位并注明阶段：P6 文件操作/选择模型接入后替换实现。
- 会话设置对话框为框架交付（比较/同步/通用三页）；同步页字段为 P8 数据位，DST/属性判据字段为 P2 数据位（置灰标注）。
- `session/resync` 引擎侧入口 P2 未开放：重连后走 UI 全量重放降级（重新 dir/open、rebindSession 重建缓存）；P2 开放后自动切换 resync 模式。
- 拖放启动在浏览器内依赖 text/plain 或 webkitGetAsEntry 相对路径；桌面端真实文件绝对路径的 onDragDropEvent 接线属 P13 Shell。
- 目录树展开/排序/过滤/文件操作/选择模型：P2/P6；文本编辑/查找/编码：P5。
