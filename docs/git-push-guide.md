# ai-compare · 提交与推送指南

> 面向所有参与 ai-compare 的同学,以及所有接手这个仓库的 AI agent。
> 目标:**看完能自己把改动安全地送上 GitHub**,
> 不再出现「push 被拒 / 超时 / 分不清该推哪个分支 / 推完 CI 红」这类来回折腾。
>
> 项目:Rust 引擎 + Tauri/Node UI 的 AI 对比工具
> 最后核对:2026-11-22(新增两条硬规则:提交标题必须带「工具/模型」前缀、推送必须用 `fengyong` 账号)。

---

## 0. 必读:两条硬规则

> 这两条没有例外。**违反其中任何一条,这次提交/推送就不算数。**

### 规则一 · 提交标题必须以「工具名 / 模型名」开头

格式:

```
<工具名>/<模型名> <类型>(<作用域>): <一句话摘要>
```

- **工具名**:你实际用的那个 agent / CLI / IDE 插件
- **模型名**:完整模型标识
- **类型**:`feat` / `fix` / `docs` / `chore` / `refactor` / `test` / `perf` / `build`
- **作用域**:不明显时可省

本机在用的前缀(照抄,或写你自己的):

| 前缀 | 工具 | 模型 |
|---|---|---|
| `mimocode/M31.1-flash` | MiniMax Code(mimocode) | MiniMax-M3.1-Flash |
| `zcode/GLM-5.3-Flash` | ZCode | GLM-5.3-Flash |
| `deepseek/DeepSeek-Flash` | DeepSeek | DeepSeek-Flash |
| `hy4/HY4-Flash` | HY4 | HY4-Flash |
| `qoder/<你在用的模型>` | Qoder | 按实际填 |

真实例子:

```
mimocode/M31.1-flash fix(ui): 比较视图认领先缓存后删除
zcode/GLM-5.3-Flash docs: 深度评审报告第 4 轮
deepseek/DeepSeek-Flash test: 补一轮契约用例
```

要点:

- 前缀必须在**标题行开头**。不能写成 `fix(ui): mimocode/M31.1-flash ...`(前缀跑到类型后面就失去意义了)。
- **换了工具或模型就换前缀。** 出问题时按前缀一筛,就能把「同一工具 / 同一模型引入的一批提交」全捞出来定位。
- 只有标题行受约束。正文(`git commit -m "标题" -m "正文"`)正常写,正文讲**为什么**,别复述 diff。
- 本仓库历史提交多数没带前缀,**从本指南生效后开始补**;新提交一律带上。

### 规则二 · 推送一律用 `fengyong` 账号

```
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/<分支>
```

为什么是它(2026-11-22 实测):

- 这台机器上 `git@github.com` 的默认身份是 `drax-ola`,**不是**规范账号;
- 本机 `~/.gitconfig` 有一条 `url.git@github.com:.insteadOf = https://github.com/`,
  会把所有 HTTPS 地址改写成 SSH —— 所以**改 remote、换 HTTPS、加 `-u` 都解决不了问题**;
- HTTPS 到 `github.com` 在这条网络下不通(`curl -I https://github.com` 超时),SSH 通;
- `~/.ssh/config` 里已经配好专属别名 `Host github-fengyong` → `~/.ssh/id_ed25519_fengyong`。

实测记录:

| 检查 | 命令 | 期望输出 |
|---|---|---|
| 默认身份是谁 | `ssh -T git@github.com` | `Hi drax-ola!` ← **别用它推** |
| 规范身份是谁 | `ssh -T git@github-fengyong` | `Hi fengyong!` |
| 写权限 | `git push --dry-run git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/main` | `Everything up-to-date`(GitHub 只对有写权限的人开放 receive-pack,能出这个结果就说明有写权限) |

**不要改 `github-fengyong` 这条别名**,它是规范账号 `fengyong` 的身份,不是你个人的。
你自己的 key 另配一条 `Host github-<你的用户名>`,但项目推送到远端时仍以 `fengyong` 身份为准。

### 两条规则合起来的完整命令

```bash
git add <显式文件路径>                       # 别用 git add -A,会误吞别人在制品
git commit -m "mimocode/M31.1-flash feat: 一句话说清做了什么"
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/<分支>
```

---

## 1. 分支模型:先搞清楚你的改动属于哪条线

| 分支 | 作用 | 谁能推 |
|---|---|---|
| `main` | 主线(引擎 + UI) | 维护者 |
| `feat/*` `review/*` | 短分支,做完整并快进合回 `main` | 各自负责人 |
| `r5-data-integrity` | 数据完整性方向的长分支 | 维护者 |

两条硬规则:

1. **不要直接在主线分支上做需求开发**,从基线切短分支。
2. **不要跨分支 merge**。需要别人的改动时用 `rebase`,不要 `git merge <别的分支>`——
   历史上出过把无关分支 merge 进 feature 分支、导致 PR 里混进上百个无关提交的事故。



---

## 2. 首次配置

### 2.1 确认仓库与写权限

`origin` 已经指向规范账号:

```bash
git remote -v
# origin  git@github-fengyong:fengyong/ai-compare.git (fetch/push)
```

写权限挂在 `fengyong` 账号上。需要用自己的身份推别的分支时,让维护者把你加成
`fengyong/ai-compare` 的 collaborator —— 但**规则二**决定你「该用谁的身份推」,
项目规范是后者:统一用 `fengyong`。

### 2.2 `fengyong` 账号的 SSH key

这台机器上已经配好了(2026-11-22 已验证):

| 东西 | 位置 |
|---|---|
| 私钥 | `~/.ssh/id_ed25519_fengyong` |
| 公钥 | `~/.ssh/id_ed25519_fengyong.pub`(已加到 `fengyong` 账号) |
| 别名 | `~/.ssh/config` 里的 `Host github-fengyong` |

换一台新机器时,从零配置:

```bash
ssh-keygen -t ed25519 -C "you@example.com" -f ~/.ssh/id_ed25519_fengyong
# 把公钥贴到 GitHub → fengyong 账号 → Settings → SSH and GPG keys → New SSH key
cat ~/.ssh/id_ed25519_fengyong.pub
```

### 2.3 配一个专属 Host 别名

编辑 `~/.ssh/config`:

```
Host github-fengyong
    HostName github.com
    User git
    IdentityFile ~/.ssh/id_ed25519_fengyong
    IdentitiesOnly yes
```

`IdentitiesOnly yes` 是关键:不加的话 SSH 可能先拿默认 key 去试,身份就串回 `drax-ola` 了。

### 2.4 验证身份(关键一步)

```bash
ssh -T git@github-fengyong     # 必须回 Hi fengyong!
ssh -T git@github.com          # 这台机器上会回 Hi drax-ola!(默认身份,别用它推)
```

如果第二条显示的账号不是你、而你又用 `git push` 直接推,就会得到
`ERROR: Permission to fengyong/<repo>.git denied to <那个人>`。

---

## 3. 标准提交流程

```bash
# 1. 从最新基线切分支
git fetch git@github-fengyong:fengyong/ai-compare.git main:refs/remotes/origin/main
git switch -c feat/my-change origin/main

# 2. 改代码 → 本地验证(见第 4 节)→ 提交
#    前缀规则见第 0 节;用显式路径 add,别用 git add -A
git add <显式文件路径>
git commit -m "mimocode/M31.1-flash feat: 一句话说清做了什么"

# 3. 推送(首次推新分支)
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/feat/my-change

# 4. 后续提交继续推同一个分支
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/feat/my-change
```

需要把别人的新提交接进来时:

```bash
git fetch git@github-fengyong:fengyong/ai-compare.git main:refs/remotes/origin/main
git rebase origin/main      # 不要 merge
# 有冲突就解决后 git add + git rebase --continue
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/feat/my-change
```

> **为什么不加 `-u`**:`git push -u <URL>` 会把 `branch.<名字>.remote` 写成那个 URL。
> 会让 `git status` 看起来有 upstream,但配置里多出一条指向别名地址的 remote,
> 后续 `git pull` / 换身份时容易互相打架。统一走显式命令更可控。

---

## 4. 提交前必须跑的检查

### 4.1 必跑(与 CI 同一套)

```bash
cargo test --locked --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cd ui && pnpm test && pnpm lint && pnpm build && pnpm format:check
```

### 4.2 改到这些文件时的额外动作

| 你改了什么 | 必须额外执行 | 为什么 |
|---|---|---|
| `ui/src-tauri/**` | `cargo test --manifest-path ui/src-tauri/Cargo.toml` + 同路径的 clippy / fmt | Tauri 侧是独立 crate,根 workspace 的命令覆盖不到 |
| 协议 / 生成绑定 | `cargo test -p ai-protocol --features ts generated_bindings_match_ui` | 生成绑定必须与 TS 侧一致,否则 UI 直接坏 |
| 依赖 | `cargo deny check` | 许可证与漏洞门禁 |

---

## 5. 提交信息规范

格式(**前缀不可省**,见第 0 节规则一):

```
<工具名>/<模型名> <类型>(<作用域>): <一句话摘要>
```

- **前缀**:工具名 / 模型名,必填,`工具/模型` 斜杠分隔
- **类型**:`feat` / `fix` / `docs` / `chore` / `refactor` / `test` / `perf` / `build`
- **作用域**:不明显时可省

真实示例:

```
mimocode/M31.1-flash feat: UI 对比视图接上引擎的新字段
mimocode/M31.1-flash fix: 比较视图认领先缓存后删除,杜绝错误路径丢编辑
mimocode/M31.1-flash docs: 补一份数据完整性方向的评审记录
```

写不下的细节放正文(`git commit -m "标题" -m "正文"`),正文讲**为什么**,别复述 diff。
正文不受前缀约束。

---

## 6. PR 与 CI

### 6.1 开 PR

推完分支后 GitHub 会打印链接,直接点;或手动:

```
https://github.com/fengyong/ai-compare/pull/new/<你的分支>
```

**base 必须选对**:AI 对比相关改动 → `main`

### 6.2 CI

本仓库有 `.github/workflows/ci.yml`。工具链、Node、pnpm 版本全部固定,所有 cargo 命令带
`--locked`,**不通过 CI 就不要推**。

| 事件 | 跑什么 |
|---|---|
| push 到 `main` | Rust 门禁 + UI 门禁 |
| PR 目标为 `main` | 同上 |
| push 到其它分支 | 不跑 CI |

也就是说:**光推 `feat/*` 分支不会有任何 CI 反馈**,必须开 PR。

---

## 7. 故障排查:push 失败的四种典型情况

### 7.1 `Permission to fengyong/ai-compare.git denied to <某人>`

说明你用的是**那个人的 SSH 身份**(这台机器默认 `git@github.com` 是 `drax-ola`)。

```bash
ssh -T git@github-fengyong              # 看它回显的是谁,应该是 fengyong
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/<分支>
```

不要去改 remote、不要换 HTTPS、不要动 `~/.gitconfig` —— 用别名地址即可。

### 7.2 `Recv failure: Operation timed out` / `Failed to connect to github.com port 443`

HTTPS 到 `github.com` 在这类网络下不通(`curl -I https://github.com` 会超时,
但 `ssh -T git@github.com` 是通的)。

```bash
# 判断:SSH 通、HTTPS 不通 → 一律走 SSH,别折腾 HTTP/1.1 或代理
ssh -T git@github-fengyong
```

换成 SSH 别名地址即可,不需要设置代理。

### 7.3 `HTTP2 framing layer` 之类

同 7.2,本质是 HTTPS 链路问题。改为 SSH 别名地址。不要靠
`-c http.version=HTTP/1.1` 硬顶,那只是偶尔能绕过,不稳定。

### 7.4 `! [rejected] ... (non-fast-forward)`

远程有你没有的提交。

```bash
git fetch git@github-fengyong:fengyong/ai-compare.git <分支>:refs/remotes/origin/<分支>
git rebase origin/<分支>
git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/<分支>
```

**永远不要用 `--force` 去碾压**(见第 8 节)。

---

## 8. 禁止事项

- ❌ **提交标题不带工具/模型前缀**(第 0 节规则一)。这是硬要求,不是建议。
- ❌ **用 `drax-ola` 或其他身份推送**(第 0 节规则二)。规范身份只有 `fengyong`。
- ❌ `git push --force` / `--force-with-lease` 到任何共享分支。
  force push 会改写别人已拉取的历史,还会让依赖提交事件触发的 CI 彻底失效。
- ❌ `git commit --amend` 一个**已经推送过**的提交(等于改写历史)。
  要修正就新开一个提交。
- ❌ `git merge <其他分支>` 合入别人的代码,用 `rebase`。
- ❌ 修改 `~/.gitconfig`(尤其是 `insteadOf` 那类全局改写)来解决单人 push 问题。
  别名地址是标准解法。
- ❌ `git add -A` 在多人 / 多 agent 共享的 worktree 里一把梭 —— 会把别人的在制品文件吞进提交。
  用显式路径。
- ❌ 把本机绝对路径、私有备份、真实业务数据提交进仓库。

---

## 9. 快速对照表

| 我想…… | 命令 |
|---|---|
| 提交(带工具/模型前缀) | `git commit -m "mimocode/M31.1-flash feat: 一句话说清做了什么"` |
| 推当前分支 | `git push git@github-fengyong:fengyong/ai-compare.git HEAD:refs/heads/<分支>` |
| 拉最新基线 | `git fetch git@github-fengyong:fengyong/ai-compare.git main:refs/remotes/origin/main` |
| 把基线接进来 | `git rebase origin/main` |
| 提交前自检 | cargo test --locked --workspace && cd ui && pnpm test && pnpm lint |
| 开 PR | `https://github.com/fengyong/ai-compare/pull/new/<分支>`(base 选对) |
| 确认推送身份 | `ssh -T git@github-fengyong` → 期望 `Hi fengyong!` |

---

## 10. 项目必读文档

| 文档 | 什么时候读 |
|---|---|
| [`README.md`](../README.md) | 架构、构建与运行、质量门禁 |
| [`docs/01-需求规格说明书.md`](../docs/01-需求规格说明书.md) | 改需求前 |
| [`docs/03-架构设计文档.md`](../docs/03-架构设计文档.md) | 动引擎/协议前 |
| [`docs/04-开发计划.md`](../docs/04-开发计划.md) | 排期与现状 |
| `PROGRESS.md` / `REVIEW*.md` | 交接与上一轮评审结论 |

> 本指南**只维护这一份,在主检出目录**。worktree 里干活时以这份为准 ——
> 同一个仓库、同一套规则,不要在 worktree 里另写一份。
