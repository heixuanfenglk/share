#!/usr/bin/env bash
# 一键生成局域网文件分享的 macOS 安装盘（.dmg）。
#
# 用法：
#   ./pack-mac.sh
#   ./pack-mac.sh --skip-build
#   ./pack-mac.sh --open

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

SKIP_BUILD=0
OPEN_DIST=0
APP_NAME="局域网文件分享"
BUNDLE_ID="local.lan-share"

usage() {
    cat <<'EOF'
用法: ./pack-mac.sh [选项]

在 macOS 上编译并生成安装盘 dist/LanShare_<版本>.dmg。
磁盘里包含应用程序，以及指向“应用程序”文件夹的快捷方式，拖进去即可安装。

选项:
  --skip-build   跳过 cargo 编译，只重新制作安装盘
  --open         完成后打开 dist 目录
  -h, --help     显示此帮助
EOF
}

step() {
    printf '\n==> %s\n' "$1" >&2
}

read_version() {
    local version
    version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
    if [[ -z "$version" ]]; then
        echo "无法从 Cargo.toml 读取版本号" >&2
        exit 1
    fi
    printf '%s\n' "$version"
}

file_bytes() {
    wc -c <"$1" | tr -d '[:space:]'
}

ensure_icon_png() {
    if [[ -f "$ROOT/assets/icon.png" ]]; then
        return
    fi
    step "生成 assets/icon.png"
    local py="python3"
    if ! command -v python3 >/dev/null 2>&1; then
        py="python"
    fi
    if ! command -v "$py" >/dev/null 2>&1; then
        echo "缺少 assets/icon.png，且未找到 Python" >&2
        exit 1
    fi
    "$py" "$ROOT/assets/gen_icon.py"
}

build_release() {
    if [[ "$SKIP_BUILD" -eq 1 ]]; then
        step "跳过 cargo 编译 (--skip-build)"
        return
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "未找到 cargo。请先安装 Rust：https://rustup.rs" >&2
        exit 1
    fi
    step "cargo build --release"
    cargo build --release --locked
}

write_icon() {
    local iconset="$1"
    local icns="$2"
    local src="$ROOT/assets/icon.png"
    rm -rf "$iconset"
    mkdir -p "$iconset"
    sips -z 16 16 "$src" --out "$iconset/icon_16x16.png" >/dev/null
    sips -z 32 32 "$src" --out "$iconset/icon_16x16@2x.png" >/dev/null
    sips -z 32 32 "$src" --out "$iconset/icon_32x32.png" >/dev/null
    sips -z 64 64 "$src" --out "$iconset/icon_32x32@2x.png" >/dev/null
    sips -z 128 128 "$src" --out "$iconset/icon_128x128.png" >/dev/null
    sips -z 256 256 "$src" --out "$iconset/icon_128x128@2x.png" >/dev/null
    sips -z 256 256 "$src" --out "$iconset/icon_256x256.png" >/dev/null
    sips -z 512 512 "$src" --out "$iconset/icon_256x256@2x.png" >/dev/null
    sips -z 512 512 "$src" --out "$iconset/icon_512x512.png" >/dev/null
    sips -z 1024 1024 "$src" --out "$iconset/icon_512x512@2x.png" >/dev/null
    iconutil -c icns "$iconset" -o "$icns"
    rm -rf "$iconset"
}

write_plist() {
    local path="$1"
    local version="$2"
    cat >"$path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh_CN</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleExecutable</key>
  <string>lan-share</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${version}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF
}

assemble_app() {
    local version="$1"
    local exe="$ROOT/target/release/lan-share"
    local app="$ROOT/dist/${APP_NAME}.app"
    local contents="$app/Contents"
    if [[ ! -f "$exe" ]]; then
        echo "未找到可执行文件: $exe" >&2
        echo "请先成功完成 cargo build --release" >&2
        exit 1
    fi
    if ! command -v sips >/dev/null 2>&1 || ! command -v iconutil >/dev/null 2>&1; then
        echo "制作图标需要 macOS 自带的 sips 和 iconutil" >&2
        exit 1
    fi

    step "组装 ${APP_NAME}.app"
    rm -rf "$app"
    mkdir -p "$contents/MacOS" "$contents/Resources"
    cp "$exe" "$contents/MacOS/lan-share"
    chmod +x "$contents/MacOS/lan-share"
    if [[ -f "$ROOT/README.md" ]]; then
        cp "$ROOT/README.md" "$contents/Resources/README.md"
    fi
    write_icon "$ROOT/dist/AppIcon.iconset" "$contents/Resources/AppIcon.icns"
    write_plist "$contents/Info.plist" "$version"

    if command -v codesign >/dev/null 2>&1; then
        codesign --force --deep --sign - "$app"
    fi
    printf '%s\n' "$app"
}

make_dmg() {
    local version="$1"
    local app="$2"
    local dist="$ROOT/dist"
    local stage="$dist/dmg-root"
    local dmg="$dist/LanShare_${version}.dmg"

    step "制作安装盘"
    rm -rf "$stage"
    mkdir -p "$stage"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    rm -f "$dmg"
    hdiutil create \
        -volname "$APP_NAME" \
        -srcfolder "$stage" \
        -ov \
        -format UDZO \
        "$dmg" >/dev/null
    rm -rf "$stage"
    printf '%s\n' "$dmg"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1 ;;
        --open) OPEN_DIST=1 ;;
        -h|--help) usage; exit 0 ;;
        *)
            echo "未知参数: $1" >&2
            usage
            exit 1
            ;;
    esac
    shift
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "这个脚本只在 macOS 上制作安装盘。" >&2
    exit 1
fi

VERSION="$(read_version)"
printf '局域网文件分享 macOS 安装盘\n'
printf '目录: %s\n' "$ROOT"
printf '版本: %s\n' "$VERSION"

ensure_icon_png
build_release
APP="$(assemble_app "$VERSION")"
DMG="$(make_dmg "$VERSION" "$APP")"

bytes="$(file_bytes "$DMG")"
printf '\n完成。\n'
printf '安装盘: %s (%.1f MB)\n' "$DMG" "$(awk -v n="$bytes" 'BEGIN { printf "%.1f", n/1048576 }')"
printf '打开安装盘后，把“%s”拖到“应用程序”。\n' "$APP_NAME"

if [[ "$OPEN_DIST" -eq 1 ]]; then
    open "$ROOT/dist"
fi
