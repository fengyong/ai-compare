#!/usr/bin/env bash
# ai-compare 桌面应用一键打包（macOS）。
#
# 流程：引擎 release → 前端产物 → Tauri 打包 .app → 随包放入引擎 → 重新签名。
#
# 为什么要「随包放入引擎」：Tauri 2 的 bundle 不自动携带引擎二进制，而桌面壳
# （ui/src-tauri/src/lib.rs）在 release 模式下按「与可执行文件同目录」解析引擎
# （macOS 即 ai-compare.app/Contents/MacOS/ai-engine），故打包后需把
# <repo>/target/release/ai-engine 复制进 .app 内并重建签名（新增可执行文件会使
# 原有签名失效）。
#
# 用法：
#   scripts/build-app.sh            # 打 .app
#   scripts/build-app.sh --dmg      # 额外打 DMG
#
# 产物：
#   ui/src-tauri/target/release/bundle/macos/ai-compare.app
#   ui/src-tauri/target/release/bundle/dmg/ai-compare_<version>_<arch>.dmg

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UI_DIR="$REPO_ROOT/ui"
TAURI_DIR="$UI_DIR/src-tauri"
APP="$TAURI_DIR/target/release/bundle/macos/ai-compare.app"
ENGINE_SRC="$REPO_ROOT/target/release/ai-engine"
WITH_DMG=0

for arg in "$@"; do
  case "$arg" in
    --dmg) WITH_DMG=1 ;;
    *) echo "未知参数: $arg" >&2; exit 2 ;;
  esac
done

# shellcheck disable=SC1091
source "$HOME/.cargo/env"

echo "==> 1/4 构建引擎（release）"
(cd "$REPO_ROOT" && cargo build --release -p ai-engine)

echo "==> 2/4 构建前端"
(cd "$UI_DIR" && pnpm build)

echo "==> 3/4 打包 Tauri 应用"
(cd "$UI_DIR" && pnpm tauri build --bundles app)

echo "==> 4/4 随包放入引擎并重新签名"
if [ ! -f "$ENGINE_SRC" ]; then
  echo "引擎产物缺失: $ENGINE_SRC" >&2
  exit 1
fi
cp "$ENGINE_SRC" "$APP/Contents/MacOS/ai-engine"
chmod +x "$APP/Contents/MacOS/ai-engine"
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true

echo
echo "✅ 完成：$APP"
echo "   引擎：$APP/Contents/MacOS/ai-engine"

if [ "$WITH_DMG" = "1" ]; then
  # 不用 `tauri build --bundles dmg`：其一，它会重建 bundle/macos/*.app，把上面
  # 随包放入的引擎覆盖掉；其二，它依赖 bundle_dmg.sh 挂载/卸载卷，在无头环境
  # 常因「资源忙」失败。改用 hdiutil create 直接以 .app 为源生成 DMG（不挂载卷）。
  VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$TAURI_DIR/tauri.conf.json" | head -1)"
  ARCH="$(rustc -vV | sed -n 's/^host: //p')"
  DMG_DIR="$TAURI_DIR/target/release/bundle/dmg"
  DMG_OUT="$DMG_DIR/ai-compare_${VERSION}_${ARCH}.dmg"
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  mkdir -p "$DMG_DIR"
  rm -f "$DMG_OUT"
  hdiutil create -volname "ai-compare" -srcfolder "$STAGE" -ov -format UDZO "$DMG_OUT" >/dev/null
  rm -rf "$STAGE"
  echo "   DMG：$DMG_OUT"
fi
