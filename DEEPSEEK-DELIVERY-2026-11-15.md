# DeepSeek Flash 交付报告（2026-11-15，D0 复核 / D1A / D1B / D2）

> 基线：开工 HEAD `b628d60`（任务书 `92fa3a0` 之后 SpaceBunny 的 D0 修复提交）。
> 核对过 SpaceBunny 三段提交（`5425de1` R3-03 / `617f370` Option / `92fa3a0` 验证基建）与
> D0 修复 `b628d60`；本轮未 reset、未改写、未 amend、未 push，SpaceBunny 历史内容零改动。
>
> 提交（worktree：`/Users/ola/drax-toolbox/ai-compare`）：
>
> | 包 | commit | subject |
> | --- | --- | --- |
> | D0 | `b628d60` | fix(dir): 修复右侧解除对齐的角色反转（**SpaceBunny 已提交，本轮仅复核**） |
> | D1A | `15ea193` | feat(protocol): 零值省略字段生成真正的可选键（ts-rs 12） |
> | D1B | `93b4eb3` | refactor(protocol): 本地重复协议类型收敛到生成类型 |
> | D2 | `b96642a` | feat(text): 增加独立文本编辑视图（FR-EDIT-001 第一阶段） |
>
> 说明：§0.2 的提交信息为**建议**文案，实际提交沿用同一语义但措辞略有差异
> （D1A 用 `feat(protocol):` 而非 `fix(protocol):`，因为除形状修正外还包含生成器升级与
> 新的护栏机制）。已形成的阶段历史不做 amend。

---

## 一、工作包 D0：右侧 unalign（复核，不重复提交）

**实际改动**：无（SpaceBunny `b628d60` 已修）。本轮仅执行真实引擎回归矩阵中可自动化的部分。

**实测证据**（`pnpm e2e:engine`，真实引擎二进制、真实临时目录）：

| 断言 | 结果 |
| --- | --- |
| 右侧入口解除后恢复为两个孤立项（`leftOnly=1 + rightOnly=1`） | ✅ |
| 解除后镜像计划恢复为「复制 + 删除」 | ✅ |
| 异名复制执行后右侧真实文件内容 | ✅ `"AAA\n"` |
| 不得按源名在右侧另建一份 / 源文件未被改写 | ✅ |

结论：D0 闭环，未发现残留缺陷。

---

## 二、工作包 D1A：非 Option 零值省略字段

### 需求依据 / 缺陷

33 个字段在 serde 侧是「零值/空值时省略键」（`default + skip_serializing_if`，条件含
`Not::not` / `Vec::is_empty` / `String::is_empty` / `is_zero_u32`），但生成的 TS 声明为
**必填键**——调用方按类型写代码会在运行时拿到 `undefined`。

完整清单（12 类型 / 33 字段）：`DirRow.hidden|suppressed`、`DirStats.dirDiff|similar`、
`TextSectionsNotification.event`、`TextManualAlignSpec.alignAnchors|isolate|neverAlignImportant|skewTolerance|proximityMatch|ignore`、
`MergeChunkDto.baseLines|leftLines|rightLines`、`GrammarItemSpec.tokens|toEol|regex|caseSensitive|orFirstLine`、
`GrammarElementSpec.caseSensitive|items`、`FileFormatSpec.masks|description|grammar|tabStops|independentLines|columnData`、
`FormatPackSpec.formats`、`RuleFormatBindingSpec.unimportantElements`、`EffectiveRulesSpec.unimportantElements`、
`RulesGetResult.formatBindings`、`RulesSetParams.formatBindings|clearFormatBindings`。

### 为什么必须动生成器（实测结论）

- ts-rs **10.1** 的 `#[ts(optional)]` 对非 Option 类型**编译期报错**
  （`ts-rs-macros-10.1.0/src/types/named.rs`：`optional` can only be used on an Option<T> type）；
  既有唯一表达 `#[ts(type = "T | undefined")]` 描述的是「键必在、值可为 undefined」，
  `{}` 仍不可赋值，且 override 会丢掉被引用类型的 import 依赖。
- ts-rs **12.0.1** 改为按 serde 属性推导：`serde(default) + skip_serializing(_if)` ⇒ `field?: T`
  （`ts-rs-macros-12.0.1/src/optional.rs`：`maybe_omitted && has_default`）。
  → 可选性成为「单一事实源」，不再需要逐字段注解，也就不会漏标。

### 实际修改文件与生产链路

- `Cargo.toml` / `Cargo.lock`：ts-rs 10.1.0 → 12.0.1（仅这两个 crate 升版 + 移除 `lazy_static`，
  **无新增依赖**）。
- `engine/ai-protocol/src/ts_export.rs`：192 处 `Type::export()` → `export(&cfg)`（v12 签名）。
- `engine/ai-protocol/src/types.rs`：移除 3 处已冗余的 `ts(type="Array<string> | undefined")`。
- 重新生成 `ui/src/protocol/generated/`：仅 14 文件变化 = 33 个字段获得 `?` + 1 处 v12 的
  HashMap 渲染变化（`DirMergePlanParams.decisions`：`{[k in string]?: V}` → `{[k in string]: V}`；
  唯一读取点用 `??` 兜底，缺键索引语义由未开启的 `noUncheckedIndexedAccess` 决定，无运行时差异；
  **未使用**已废弃的 `with_v11_hashmap` 兼容开关）。
- 生产链路的真实兜底（无 `as`/`as unknown as` 绕过）：`formatsModel.ts` 新增
  `masksOf/grammarOf/itemsOf` 集中读取器（替换 15 处直读）、`FileFormatsPage.tsx`、
  `rulesModel.ts`、`DirStatsBar.tsx`、`mockEngine.ts`。

### 新增测试 / 修复前失败证据

- `option_shape.rs` 新增 `generated_ts_field_shapes_match_serde_attributes`：
  逐字段对账源码形状与**实际生成物**（`skip ⇔ 有 ?`；非 skip ⇔ 无 ?；非 Option 省略字段必须带
  `serde(default)`，否则自身输出无法被自身反序列化）。
- 新增 `zero_value_omitted_fields_round_trip`：零值省略键 / 非零保留键 / 显式零值可解析 /
  `null` 不被接受（可选键 ≠ nullable）。
- **变异验证**（三条方向全部变红，恢复后变绿）：
  去掉 `DirStats.dir_diff` 的 `serde(default)` → 报「与自身输出不对称」；
  去掉 `FileFormatSpec.masks` 的 `skip_serializing_if` → 报「生成物是可选键」；
  手工抹掉 `DirRow.hidden` 生成物的 `?` → 报「线上会省略该键，但生成的 TS 不是可选键」。
- UI 侧 `tsc` 暴露的真实漂移全部修正（例如 `DirStatsBar` 读 `stats.similar`、
  `FileFormatsPage` 读 `format.grammar.map`）。

**线上 JSON 值零变化**（未改任何 serde 属性）；旧客户端载荷仍被接受。

---

## 三、工作包 D1B：本地重复协议类型收敛

### 盘点方法（可复现）

以 `generated/` 的 192 个类型名为基准，扫描 `ui/src` 其余源码中的同名 `type`/`interface`
声明（排除 `import { type X }` 内联导入）。结果 8 处：

| 位置 | 类型 | 与生成类型的真实漂移 |
| --- | --- | --- |
| `protocol/types.ts` | `DirExecuteOp/Params/OpStatus/OpResult/Result` | `detail` 写成「必填可空」，真相是键可省略；`targetSide/targetPath/newName` 写成 `| null`；succeeded 项缺 `status`；缺 `DirExecuteOpKind` 命名 |
| `lib/inlineDiffCore.ts` | `CharRange` | 与 `generated/CharRange` 完全同形 |
| `views/text-compare/cmDecorations.ts`、`overview.ts` | `Importance` ×2 | 与 `generated/Importance` 完全同形，且协议值直接流入 |

### 改动

- 全部改为生成类型唯一来源，对外保留原导出名（`export type { X }`），调用方零改动。
- `syncPlan.reportFromExecute` 参数由手写镜像改为 `Omit<DirExecuteResult, 'sessionId'>`。
- 按真实形状修正消费点（不做 cast）：mock 引擎 5 处 `detail: null` → 不带该键；
  明细渲染 `d.detail ?? null` / `d.detail ?? ''`；测试夹具补齐协议必填的 `status`。
- 新增护栏 `ui/src/protocol/mirrorGuard.test.ts`：`generated/` 之外出现同名协议类型声明即失败
  （用 `import.meta.glob(?raw)` 读源码，避免渲染层 tsconfig 引入 Node 类型）。
  变异验证：注入 `export type DirExecuteOp = …` 立即变红。
- `as unknown as` 共 13 处**逐处核验仍必要**（泛型键合并 / 动态记录索引），未删除、未新增。

---

## 四、工作包 D2：独立文本编辑视图（FR-EDIT-001 第一阶段）

### 需求依据

SRS `FR-EDIT-001` 第一阶段：单窗格文本编辑器，复用真实引擎的编码与保存能力。
任务书 D2.2 的九项最低生产闭环、D2.3 的四条架构约束、D2.4 的最低测试矩阵。

### 关键设计判断（为什么不是「复用双栏」）

单文件编辑没有「另一侧」：对齐视图的填充行、差异段、双侧模型级撤销栈在单文件场景
都是不存在的概念。因此复用**文本能力与纯核心**（`text/open`、`text/lines`、`text/edit`、
`text/save`、`lib/findCore`、`langPacks`、引擎的编码/BOM/行尾权威），另建单窗格 CM6 宿主
（`editablePane.ts`，原生撤销/重做/剪贴板）与单侧编辑模型（`editModel.ts`），
**不复用 `AlignedPane`**。

### 协议改动（向后兼容，已有能力位门控）

| 改动 | 语义 | 兼容性 |
| --- | --- | --- |
| `TextOpenParams.right: String` → `Option<String>` | **省略 = 单文件会话**：只建左表、不读右文件、不跑 diff；`rightLineCount=0`、`rightEncoding=""` | 旧客户端始终携带 `right`，逐字节不变（e2e 专断言）；旧引擎缺省 right 会 412，故 UI 先探测能力位 |
| `EngineCapabilities.textOpenOne: Option<bool>`（缺省 false） | 声明支持单侧 open | 新客户端对旧引擎退回「同路径双开」（内容等价，仅多一次读盘 + 一次 identical diff） |
| `TextSessionData.single_side`（内部） | 单文件会话的 `text/edit` 跳过重 diff | 不影响旧会话 |

不存在的右侧用「空编码标签 + 无 BOM」占位：任何误写都会因标签非法而失败，
**不会静默把文件写成 UTF-8**。

### 生产链路

1. 入口：主页「文本编辑」大按钮、拖放单文件、目录行右键「用文本编辑打开」（取
   `leftPath/rightPath` 真实地址，不用显示 id）、文本比较工具栏「用文本编辑打开」。
2. 打开：`openEditTab` → `text/open`（省略 right）→ 标签 `kind:'edit'`；
   视图 `text/lines` 左侧分页拉全 → CM6 单窗格。
3. 编辑：CM 事务 → `editModel.computeLineEdit`（公共前后缀裁剪的最小行区间）→
   `text/edit`（防抖 200ms，串行）。
4. 保存：`saveEditDoc` = **先收敛（`syncDocToEngine`）再 `text/save side=left`**；
   工具栏、`Ctrl+S`（`saveSessionForTab` 对编辑标签分流）、关闭守卫三处共用同一份实现。
   另存为走 `leftPath` 并接受引擎回执改指；备份由 `backup` 开关控制（默认 true）。
5. 关闭：复用既有 content 守卫（`contentDirty.left`）——取消保留标签与内容；
   保存失败不清脏、不关闭；原生窗口退出走同一批 `pendingClose` 续跑。
6. 崩溃重放：`TAB_SESSION_KINDS` 纳入 `edit`；重绑后按路径认领旧会话的未落盘编辑
   （`findDirtyEditDocByPath`），以引擎真值为收敛基准重新提交。
7. 命令体系：`edit.find`（Ctrl+F）、`edit.toggleWrap`、`edit.saveFile`、`edit.saveFileAs`
   （键位说明见代码注释：`Ctrl+S` 已被 `session.save` 占用，静态冲突检测不看 `when` 门控，
   因此编辑标签的 Ctrl+S 由会话保存分流，不新增冲突键）。

### 新增测试（本包 41 条 UI + 4 条引擎 + 19 条 e2e 断言）

| 文件 | 覆盖 |
| --- | --- |
| `text-edit/editModel.test.ts`（18） | 行数组⇄文档文本可逆（含末行空行/空文件歧义）、最小区间、`from→to` 全组合应用一致性、收敛（ok/重放/mismatch/unsupported）、并发串行、按路径认领、缓存 |
| `text-edit/textEditSave.test.ts`（8） | 先收敛再落盘、另存为改指、413 编码错误保留脏、412 路径错误、`text/save` 缺失不假报、mismatch 阻止保存（不写陈旧内容） |
| `text-edit/editablePane.test.ts`（6） | 真实 CM6：编辑回调、程序化回灌抑制、替换/撤销/重做、光标坐标、只读、换行 |
| `text-edit/TextEditView.test.tsx`（7） | 单侧 open 参数、旧引擎退回、编辑→脏标记→`text/edit` 载荷、保存成功/失败、只读降级、崩溃重放认领 |
| `shell/editTabFlow.test.ts`（10） | 打开参数/标签数据、会话规格（`left` + 空 `right`）、会话树重开、content 守卫取消/保存/失败、Ctrl+S 分流、崩溃重绑 |

引擎侧（`engine/ai-engine/src/handlers.rs`）：单侧 open 不建右表/不跑 diff、
GB18030+CRLF+末行无换行原样回写（字节级）、UTF-16LE+BOM 保留、另存为改指且不改源文件。

真实引擎 e2e（`ui/scripts/e2e-engine.mjs` 3d 段）：UTF-16LE+BOM+CRLF 语料的
open/lines/edit/save/backup/重开/另存为/412 失败/旧客户端带 `right` 兼容性，共 19 条断言，
落盘字节逐字节比对。

### 真实运行输入 → 动作 → 最终状态（e2e 实测）

输入：文件 `alpha\r\nbeta`（UTF-16LE + BOM，末行无换行）。
动作：`text/open{left}` → `text/edit{side:left,start:2,text:"\n"}` → `text/save{side:left}`。
最终状态：文件字节 = `FF FE` + `"alpha\r\nbeta\r\n"`（UTF-16LE），`.bak` = 原始字节；
`leftModified=false`；重开 `leftLineCount=2`；在末行继续输入 `gamma` 后内容正确进入引擎；
另存为写出正确字节且源文件不变；写不存在目录 → `RPC 412`，源文件字节不变。

### 已知边界（不遮掩）

行数组口径无法表达「末行为空且无行尾符」，因此**在最后一行按 Enter 等价于补一个行尾符**：
落盘字节正确（e2e 断言），但重开后引擎行数比编辑时少 1。彻底消除需协议再暴露
「末行是否有行尾符」，属协议扩展，本阶段不做；已在 `editModel.ts` 模块注释与 e2e 断言中写明。

---

## 五、全量门禁实测（本轮）

```text
cargo test --locked --workspace                                   1057 passed / 0 failed   （基线 1051，+6）
cargo clippy --locked --workspace --all-targets -- -D warnings     零告警
cargo fmt --all -- --check                                        0 diff
cargo deny check                                                  advisories/bans/licenses/sources ok
cargo test -p ai-protocol --features ts generated_bindings_match_ui   passed（1）
cargo test --manifest-path ui/src-tauri/Cargo.toml                19 passed / 0 failed
cargo clippy --manifest-path ui/src-tauri/Cargo.toml … -D warnings  零告警
cargo fmt --manifest-path ui/src-tauri/Cargo.toml --all -- --check  0 diff
pnpm test                                                         824 passed / 0 failed（81 文件；基线 772/75）
pnpm lint / build / format:check                                  全绿
npx tsc -b --force                                                0 错
pnpm e2e:engine                                                   全过（含 D2 段 19 条断言）
pnpm verify:demo                                                  全过
git diff --check                                                  干净（见下）
```

`git diff --check` 说明：ts-rs 生成物在「字段后跟文档注释」时行尾固定带一个空格，
整个 `generated/` 目录 98 个文件一致、可逐字节复现；手工改这些空格会让再生成必然失败。
因此新增 `.gitattributes` **只对** `ui/src/protocol/generated/*.ts` 关闭行尾空白检查，
其余源码仍受约束。

---

## 六、协议兼容与影响范围

- **线上 JSON 值**：D1A 未改任何 serde 属性；D2 只把 `TextOpenParams.right` 变为可选
  （旧载荷照旧）。golden 快照仅新增一行 `"textOpenOne": true`（3 个文件 + 协议回放 fixture）。
- **生成绑定**：`TextOpenParams.ts`（`right?: string`）、`EngineCapabilities.ts`（`textOpenOne?: boolean`）。
- **能力协商**：新 UI 缺 `textOpenOne` 时退回同路径双开；缺 `textEdit` 时只读；缺 `textSave` 时
  保存按钮提示不支持（不假报）。
- **影响进程**：引擎单进程 `ai-engine`；UI 内新增视图与会话类型，不影响既有视图链路。

---

## 七、未验证 / 未完成

1. **D3（只读补丁视图）未开始**——任务书标注为可选包，且 §0.3-9 要求「Token 不足时完整交付
   当前包后停止，不留下多个基本完成」。D0～D2 均已独立验收。
2. **真实桌面 WebView 的 GUI 未实测**：本包证据等级为「真实引擎 e2e（真实字节）+
   jsdom 组件测试（真实 CM6 实例）」。人工清单见第九节，**标注未实测**。
3. 编辑视图**不支持会话内切换编码**（保留打开时编码，与「保留原编码」的需求一致；
   需要换编码仍可用文本比较视图的既有入口）。
4. 多文件搜索、SCC 签入签出不属本阶段（任务书明示）。
5. 非 macOS 平台、中文 IME 输入法在编辑视图中的行为未实测。

---

## 八、新发现问题（本轮发现，未擅自修）

1. **`text/open` 无「会话已存在」检查**：重复 open 会静默覆盖会话并把 `rev` 归零
   （`dir/open` 有 411）。UI 侧对文本会话的 411 兜底分支在真实引擎上永不触发。
   属既有行为，未在本轮改动（涉及会话语义决策）。
2. **`text/edit` 无法表达「在末行新增空行」**：`LineTable::from_text("")` = 0 行，
   插入空行必须把 text 写成 `"\n"`；双栏比较视图的 `opsFromPatches` 仍用
   `inserted.join('\n')`，遇到空行插入时会触发重放→分叉→回落客户端重 diff
   （保存时该空行会丢）。本轮只在**编辑视图**用 `linesToEditText` 修正，未动比较视图
   （属另一包的回归风险，需单独评估）。
3. **生成物行尾空格**：ts-rs 12 在「字段后跟文档注释」时输出 `, ` 结尾，
   与 `git diff --check` 冲突；已用 `.gitattributes` 精准豁免（见第五节）。

---

## 九、人工 GUI 清单（未实测，供真机验收）

编辑器（桌面版）：

1. 主页填一个文本文件路径 → 点「文本编辑」大按钮 → 应新开标签并显示内容、编码徽标、行数。
2. 目录比较中右键某文件→「用文本编辑打开」→ 应以该侧真实文件（不是显示名）打开。
3. 编辑若干内容 → 标签出现脏点、状态栏行数变化 → 关闭标签 → 应弹「保存/不保存/取消」；
   选「取消」→ 标签与内容保留。
4. 选「保存」→ 提示含 `.bak` 路径 → 磁盘出现 `<文件>.bak`（内容为编辑前）→ 脏点消失。
5. 关闭引擎进程（或 `kill`）→ UI 出现重连横幅 → 恢复后编辑内容仍在 → 再次保存应成功。
6. 直接点窗口关闭按钮 → 与第 3 步同样的未保存提示（批量：应逐个处理）。
7. 「查找」按钮 / Ctrl+F → 输入关键字 → 回车逐个定位（选区高亮）；「全部替换」后保存，
   磁盘内容应同步。
8. 只读降级：用不含 `text/edit` 的旧引擎启动 → 应显示只读徽标与原因，保存按钮禁用。

---

## 十、变更是否已拆分

| 包 | 是否独立提交 | 是否仅含本包文件 |
| --- | --- | --- |
| D0 | 是（SpaceBunny `b628d60`） | 本轮零改动 |
| D1A | 是（`15ea193`） | 25 文件，全部属 D1A（依赖升级 + 生成物 + 形状消费点 + 护栏） |
| D1B | 是（`93b4eb3`） | 11 文件，全部属 D1B；未夹带格式重排 |
| D2 | 是（`b96642a`） | 40 文件（引擎协议/处理器/测试 + UI 视图/接线/样式/e2e + `.gitattributes`） |
| 文档 | 单独一个 commit | 仅 `PROGRESS.md` 与本报告 |

未跟踪的评审/任务书文档、`ai_lair/`、`.tmp/` 与本轮实现无关，**未暂存、未提交**；
本轮未新增临时探针（试验用的 ts-rs 源码副本在 `target/` 内，已清理）。
