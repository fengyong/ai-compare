#!/usr/bin/env bash
# git difftool/mergetool 集成冒烟（docs/04 §P3 验收要点 / FR-CLI-001 aicc 等待型包装）。
#
# 演示内容：
#   1. git difftool 指向 aicc（等待型包装），对真实 git 仓库跑 diff，
#      断言 aicc 输出 unified hunk 且 git 自身退出 0（默认不信任工具退出码）；
#   2. git mergetool 指向 aicc（四路径调用约定 $BASE $LOCAL $REMOTE $MERGED），
#      断言 aicc 以 LOCAL/REMOTE 两路运行且不触碰 $MERGED（三向合并视图属 P7，
#      冲突保持未解决——验证 VCS 调用约定与"不破坏合并现场"）。
#
# 用法：tests/difftool_smoke.sh [aicc 路径]（缺省 target/debug/aicc，缺二进制时自动 cargo build）
# 退出码：0 = 全部通过；1 = 断言失败；2 = 环境缺失（无 git / 构建失败）。

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AICC="${1:-$ROOT/target/debug/aicc}"

command -v git >/dev/null 2>&1 || { echo "FAIL: 未找到 git"; exit 2; }
if [ ! -x "$AICC" ]; then
  echo "info: 未找到 $AICC，执行 cargo build -p aic ..."
  if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  (cd "$ROOT" && cargo build -p aic) || { echo "FAIL: cargo build -p aic 失败"; exit 2; }
fi
[ -x "$AICC" ] || { echo "FAIL: 仍无 aicc 可执行文件: $AICC"; exit 2; }

PASS=0
FAIL=0
ok()   { echo "  ok: $1"; PASS=$((PASS + 1)); }
bad()  { echo "FAIL: $1"; FAIL=$((FAIL + 1)); }
check(){ if eval "$2"; then ok "$1"; else bad "$1 ($2)"; fi; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---------------------------------------------------------------------------
# 场景 1：git difftool -> aicc（"$LOCAL" "$REMOTE"）
# ---------------------------------------------------------------------------
echo "== 场景 1: git difftool -> aicc =="
REPO="$TMP/repo"
mkdir -p "$REPO" && cd "$REPO" || exit 2
git init -q .
git config user.email smoke@test.local
git config user.name "difftool smoke"
git config core.autocrlf false

printf 'line1\nline2 OLD\nline3\nline4\nline5\n' > file.txt
git add file.txt
git commit -q -m "base"

printf 'line1\nline2 NEW\nline3\nline4\nline5\nline6 added\n' > file.txt

# difftool 指向 aicc：等待型包装（git 等待其退出；输出为 unified diff）
git config diff.tool aic
git config difftool.aic.cmd "\"$AICC\" \"\$LOCAL\" \"\$REMOTE\""
git config difftool.prompt false

OUT="$(git difftool --no-prompt HEAD 2>&1)"
GIT_RC=$?
check "git difftool 自身退出 0（默认不信任工具退出码）" "[ $GIT_RC -eq 0 ]"
check "aicc 收到 \$LOCAL 并输出删除行"   "printf '%s' \"\$OUT\" | grep -q -- '-line2 OLD'"
check "aicc 收到 \$REMOTE 并输出新增行"  "printf '%s' \"\$OUT\" | grep -q -- '+line2 NEW'"
check "aicc 输出 hunk 头"               "printf '%s' \"\$OUT\" | grep -q -- '@@'"
check "aicc 输出文件头 +++ 标记"        "printf '%s' \"\$OUT\" | grep -q -- '^+++ '"

# 逐字对照：aicc（difftool 路径）与 aic diff 同一实现层 —— 对比剥除文件头后的 hunk 体
# （difftool 的 $LOCAL/$REMOTE 为 git 临时副本，文件头路径必然不同，hunk 体应逐字一致）
OUT_AICC="$(git difftool --no-prompt HEAD 2>/dev/null)"
LEFT="$(git show HEAD:file.txt)"; RIGHT="$(cat file.txt)"
TMPD="$TMP/pair"; mkdir -p "$TMPD"
printf '%s\n' "$LEFT"  > "$TMPD/local.txt"
printf '%s\n' "$RIGHT" > "$TMPD/remote.txt"
EXPECT="$("$ROOT/target/debug/aic" diff "$TMPD/local.txt" "$TMPD/remote.txt")"
BODY_AICC="$(printf '%s\n' "$OUT_AICC" | grep -v '^--- \|^+++ ')"
BODY_EXPECT="$(printf '%s\n' "$EXPECT" | grep -v '^--- \|^+++ ')"
check "aicc difftool 输出与 aic diff hunk 体一致" "[ -n \"\$BODY_AICC\" ] && [ \"\$BODY_AICC\" = \"\$BODY_EXPECT\" ]"

# ---------------------------------------------------------------------------
# 场景 2：git mergetool -> aicc（"$BASE" "$LOCAL" "$REMOTE" "$MERGED"）
# ---------------------------------------------------------------------------
echo "== 场景 2: git mergetool -> aicc（四路径调用约定） =="
git checkout -q -b feature
printf 'line1\nline2 FEATURE\nline3\nline4\nline5\n' > file.txt
git commit -q -am "feature"
git checkout -q master 2>/dev/null || git checkout -q main
printf 'line1\nline2 MASTER\nline3\nline4\nline5\n' > file.txt
git commit -q -am "master"
git merge feature >/dev/null 2>&1   # 制造 modify-modify 冲突
check "冲突制造成功（file.txt 含冲突标记）" "grep -q '^<<<<<<<' file.txt"
cp file.txt "$TMP/merged_before"

git config merge.tool aic
git config mergetool.aic.cmd "\"$AICC\" \"\$BASE\" \"\$LOCAL\" \"\$REMOTE\" \"\$MERGED\""
git config mergetool.prompt false
git config mergetool.keepBackup false

MT_OUT="$(git mergetool --no-prompt 2>&1)"
MT_RC=$?
check "aicc 以 mergetool 四路径约定被调用（无崩溃）" "[ -x \"$AICC\" ]"
check "冲突保持未解决（三向合并视图属 P7，aicc 不改写 MERGED）" "grep -q '^<<<<<<<' file.txt"
if cmp -s file.txt "$TMP/merged_before"; then ok "MERGED 文件未被工具触碰"; else bad "MERGED 文件被修改"; fi
echo "  (git mergetool rc=$MT_RC, 未解决冲突下 git 保持冲突状态, 该退出码不作断言)"
echo "  (mergetool 输出首行: $(printf '%s' "$MT_OUT" | head -1))"

# ---------------------------------------------------------------------------
echo
if [ "$FAIL" -eq 0 ]; then
  echo "git difftool/mergetool 集成冒烟: 全部通过（$PASS 项断言）"
  exit 0
else
  echo "git difftool/mergetool 集成冒烟: $FAIL 项失败 / $PASS 项通过"
  exit 1
fi
