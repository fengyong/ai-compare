#!/usr/bin/env bash
# ai-compare 看门狗（本地 10 分钟层）
#
# 每 10 分钟检查「当前开发任务是否仍在继续」：
#   - 最近一次 git commit / 源码落盘时间
#   - cargo / pnpm / vitest / ai-engine 进程活性
#   - 网络连通（两次探活）
#   - 引擎 stdio 探活（若 ai-engine 常驻）
# 正常：写心跳 target/watchdog/heartbeat.json
# 异常：追加信号 target/watchdog/SIGNAL.md，由每小时的 IDE automation（AI 接手层）
#       读取后从 PROGRESS.md 登记的断点自动继续；处理完由 automation 清空信号。
#
# 说明：IDE automation 的 RRULE 最小粒度为 1 小时（HOURLY），BYMINUTE/多值
#       BYHOUR 均不展开，因此 10 分钟检查由本守护进程承担，AI 自动继续由
#       automation 承担（信号驱动，最长 1 小时接续延迟）。
#
# 用法：nohup bash scripts/watchdog.sh > target/watchdog/daemon.log 2>&1 &
#       WATCHDOG_INTERVAL=600 WATCHDOG_STALE=900 可覆盖（单位是秒）
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/target/watchdog"
INTERVAL="${WATCHDOG_INTERVAL:-600}"
STALE="${WATCHDOG_STALE:-1800}"
mkdir -p "$OUT"
cd "$ROOT" || exit 1
SELF=$$

network_ok() {
  curl -sS -m 3 -o /dev/null https://www.apple.com 2>/dev/null && return 0
  sleep 2
  curl -sS -m 3 -o /dev/null https://www.baidu.com 2>/dev/null && return 0
  return 1
}

active_procs() {
  # 只把「开发在推进」的进程算作活跃：cargo test/clippy/build、vitest、
  # pnpm test/build/lint/e2e、ai-engine；排除常驻 dev server 与 IDE 宿主进程，
  # 否则 pnpm dev 常驻会让停滞判定永远失效。
  pgrep -fl '(cargo (test|clippy|build)|vitest|pnpm (test|build|lint|e2e)|ai-engine)' 2>/dev/null \
    | grep -v -E 'watchdog\.sh|app\.asar|DeepSeek Harness' | head -8 || true
}

check_once() {
  local now ts head last_commit newest_src activity age procs dirty net reasons status
  now=$(date +%s)
  ts=$(date "+%Y-%m-%d %H:%M:%S %z")
  head=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
  last_commit=$(git log -1 --format=%ct 2>/dev/null || echo 0)
  newest_src=$(find ui/src engine cli -type f \
    \( -name '*.ts' -o -name '*.tsx' -o -name '*.rs' -o -name '*.mjs' \) \
    -exec stat -f %m {} + 2>/dev/null | sort -n | tail -1)
  newest_src=${newest_src:-0}
  activity=$last_commit
  if [ "$newest_src" -gt "$activity" ] 2>/dev/null; then activity=$newest_src; fi
  age=$(( now - activity ))
  procs=$(active_procs)
  dirty=$(git status --porcelain 2>/dev/null | wc -l | tr -d ' ')
  reasons=""
  status=ok

  if ! network_ok; then
    status=anomaly
    reasons="${reasons}网络中断(两次探活失败); "
  fi
  if [ "$age" -gt "$STALE" ] && [ -z "$procs" ]; then
    # 二次复验：等 30 秒再看活动时间戳，避免把刚落盘的进展误判为停滞
    sleep 30
    local activity2 newest2
    activity2=$(git log -1 --format=%ct 2>/dev/null || echo 0)
    newest2=$(find ui/src engine cli -type f \
      \( -name '*.ts' -o -name '*.tsx' -o -name '*.rs' -o -name '*.mjs' \) \
      -exec stat -f %m {} + 2>/dev/null | sort -n | tail -1)
    newest2=${newest2:-0}
    if [ "$newest2" -gt "$activity2" ] 2>/dev/null; then activity2=$newest2; fi
    if [ "$activity2" -le "$activity" ]; then
      status=anomaly
      reasons="${reasons}${age}s 无提交/源码落盘且无构建进程(二次复验确认), 疑似会话中断或服务端停止响应; "
    fi
  fi
  if [ "$dirty" -gt 0 ] && [ "$age" -gt "$STALE" ] && [ -z "$procs" ]; then
    reasons="${reasons}工作树有 ${dirty} 个未提交改动且长时间无推进; "
  fi

  printf '{"ts":"%s","epoch":%s,"head":"%s","lastCommit":%s,"newestSource":%s,"idleSeconds":%s,"dirty":%s,"procs":%s,"status":"%s"}\n' \
    "$ts" "$now" "$head" "$last_commit" "$newest_src" "$age" "$dirty" \
    "$(printf '%s' "$procs" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')" \
    "$status" > "$OUT/heartbeat.json"

  if [ "$status" = anomaly ]; then
    printf -- '- %s HEAD=%s idle=%ss dirty=%s 异常: %s\n' \
      "$ts" "$head" "$age" "$dirty" "$reasons" >> "$OUT/SIGNAL.md"
  fi
  printf '%s %s head=%s idle=%ss dirty=%s %s\n' "$ts" "$status" "$head" "$age" "$dirty" "$reasons" >> "$OUT/check.log"
}

trap 'exit 0' TERM INT
while true; do
  check_once
  sleep "$INTERVAL"
done
