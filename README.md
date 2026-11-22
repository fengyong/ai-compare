# ai-compare

跨平台文件/文件夹/文本/表格/十六进制/图片/三向合并比较工具——完整复刻 Beyond Compare（BC5）的功能与交互。

> 产品原则：一切交互、视觉语义、快捷键、退出码对齐 Beyond Compare（验收基准见 `docs/01-需求规格说明书.md`，163 个 FR，基于 BC 5.2.0 官方帮助文档逆向）。

## 架构

Rust headless 引擎（库 + `ai-engine --stdio` 独立进程）+ Tauri2/React UI + `aic` CLI 三形态同核，NDJSON JSON-RPC over stdio（LSP 模式）。

```
engine/   Rust workspace：ai-core(diff/merge3/table/hex/picture/report) · ai-dir(目录/同步/执行) · ai-vfs(本地/zip/FTP) · ai-protocol · ai-engine
cli/      aic(diff/quick/merge/table/hex/patch/report/script) · aicc(VCS difftool/mergetool 等待型包装) · aiscript(脚本处理器)
ui/       Tauri2 + React18：目录双窗格/文本/表格/十六进制/图片比较 + 三向合并 + 同步 + 会话树/设置框架
tests/    golden 快照 · difflib/git/xxd/csvkit 多方对拍矩阵 · 协议录制回放 · 性能基准(PB-03/05/10)
docs/     需求规格(163 FR) · 竞品调研 · 架构设计(10 ADR) · 开发计划(P0~P17)
```

## 构建与运行

```bash
# 引擎（Rust 1.88+）
cargo build --workspace
cargo test --workspace          # 822 测试

# CLI 冒烟
target/debug/aic quick <dir1> <dir2>      # 目录快速比较（退出码 0/1/2/11-14）
target/debug/aic diff <f1> <f2>           # unified diff（与 diff -u 等价）
target/debug/aic merge <base> <l> <r>     # 三向合并（与 git merge-file 对拍 100% 一致）

# UI（Node 20+ / pnpm）
cd ui && pnpm install
pnpm dev            # 浏览器演示模式（内置演示引擎）
pnpm tauri dev      # 桌面端（spawn target/debug/ai-engine）

# 质量门禁
cargo clippy --workspace --all-targets -- -D warnings
cd ui && pnpm build && pnpm test && pnpm e2e:engine
```

## 性能（PB 基准，release）

| 项 | 结果 |
| --- | --- |
| PB-03 十万项目录扫描 | 1.54s（目标 ≤2s） |
| PB-10 delta 序列化 | 10 万行 23ms |
| PB-05 20 万行 diff | 24.9ms（standard） |

## 状态

引擎 822 + UI 400 测试全绿。BC 忠实度审计清单与迭代队列见 `PROGRESS.md`。

## 提交与推送（必读）

**两条硬规则**,任何一次提交和推送都必须满足,完整版见
[`docs/git-push-guide.md`](docs/git-push-guide.md):

1. **提交标题必须以「工具名 / 模型名」开头**

   ```bash
   git commit -m "mimocode/M31.1-flash feat: 一句话说清做了什么"
   ```

   格式:`<工具名>/<模型名> <类型>(<作用域>): <一句话摘要>`。前缀取值 `mimocode/M31.1-flash`
   (MiniMax Code / MiniMax-M3.1-Flash)、`zcode/GLM-5.3-Flash`、`deepseek/DeepSeek-Flash` 等,
   换工具或换模型就写当次实际用的那个。前缀必须在标题行**开头**。

2. **推送一律用 `fengyong` 账号**

   ```bash
   git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/<分支>
   ```

   这台机器上 `git@github.com` 的默认身份是 `drax-ola`、对本仓库无写权限;
   `~/.gitconfig` 的 `insteadOf` 全局改写让「改 remote / 换 HTTPS / 加 `-u`」全部无效。
   `~/.ssh/config` 里已配好 `Host github-fengyong` 别名,用 `ssh -T git@github-fengyong`
   校验应回 `Hi fengyong!`。
