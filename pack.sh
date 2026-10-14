#!/usr/bin/env bash
# 一键生成局域网文件分享的 Windows 安装包。
# macOS / Linux：交叉编译 + NSIS
# Windows：转交 packaging/build.ps1（Inno Setup）
#
# 用法：
#   ./pack.sh
#   ./pack.sh --skip-build
#   ./pack.sh --open

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

SKIP_BUILD=0
OPEN_DIST=0
CACHE="${HOME}/.cache/lan-share-pack"
LLVM_MINGW_TAG="20261006"
NSIS_VERSION="3.11"

TARGET=""
TOOLCHAIN_BIN=""
LINKER=""
CC_BIN=""
CXX_BIN=""
AR_BIN=""
WINDRES_BIN=""
OBJDUMP_BIN=""
MAKENSIS=""

usage() {
    cat <<'EOF'
用法: ./pack.sh [选项]

一键编译并生成 Windows 安装包 dist/LanShare_Setup_<版本>.exe

选项:
  --skip-build   跳过 cargo 编译，只重新制作安装包
  --open         完成后打开 dist 目录
  -h, --help     显示此帮助

Windows 上优先使用 Inno Setup（packaging/build.ps1）。
macOS / Linux 上交叉编译 Windows 程序，并用 NSIS 制作安装包。
本机没有可用的 MinGW / NSIS 时，脚本会把工具下载到 ~/.cache/lan-share-pack。
EOF
}

step() {
    printf '\n==> %s\n' "$1"
}

is_windows() {
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*|Windows*) return 0 ;;
        *) return 1 ;;
    esac
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

version_quad() {
    local version="$1"
    case "$version" in
        *.*.*.*) printf '%s\n' "$version" ;;
        *.*.*) printf '%s.0\n' "$version" ;;
        *.*) printf '%s.0.0\n' "$version" ;;
        *) printf '%s.0.0.0\n' "$version" ;;
    esac
}

file_bytes() {
    wc -c <"$1" | tr -d '[:space:]'
}

cpu_count() {
    if command -v sysctl >/dev/null 2>&1; then
        sysctl -n hw.ncpu 2>/dev/null && return
    fi
    if command -v nproc >/dev/null 2>&1; then
        nproc && return
    fi
    echo 4
}

download_file() {
    local url="$1"
    local dest="$2"
    if [[ -s "$dest" ]]; then
        return
    fi
    mkdir -p "$(dirname "$dest")"
    step "下载 $(basename "$dest")"
    curl -fL --retry 3 --retry-delay 2 -o "$dest.partial" "$url"
    mv "$dest.partial" "$dest"
}

brew_can_install() {
    local prefix
    command -v brew >/dev/null 2>&1 || return 1
    prefix="$(brew --prefix 2>/dev/null || true)"
    [[ -n "$prefix" && -w "$prefix" && -w "$prefix/Homebrew" ]]
}

ensure_icon() {
    local icon="$ROOT/assets/icon.ico"
    if [[ -f "$icon" ]]; then
        return
    fi
    step "生成 assets/icon.ico"
    local py="python3"
    if ! command -v python3 >/dev/null 2>&1; then
        py="python"
    fi
    if ! command -v "$py" >/dev/null 2>&1; then
        echo "缺少图标 assets/icon.ico，且未找到 Python" >&2
        exit 1
    fi
    "$py" "$ROOT/assets/gen_icon.py"
}

use_gnu_toolchain() {
    TARGET="x86_64-pc-windows-gnu"
    LINKER="x86_64-w64-mingw32-gcc"
    CC_BIN="x86_64-w64-mingw32-gcc"
    CXX_BIN="x86_64-w64-mingw32-g++"
    AR_BIN="x86_64-w64-mingw32-ar"
    WINDRES_BIN=""
    if command -v x86_64-w64-mingw32-objdump >/dev/null 2>&1; then
        OBJDUMP_BIN="x86_64-w64-mingw32-objdump"
    fi
}

use_gnullvm_toolchain() {
    local bin="$1"
    TARGET="x86_64-pc-windows-gnullvm"
    TOOLCHAIN_BIN="$bin"
    LINKER="$bin/x86_64-w64-mingw32-clang"
    CC_BIN="$bin/x86_64-w64-mingw32-clang"
    CXX_BIN="$bin/x86_64-w64-mingw32-clang++"
    if [[ -x "$bin/llvm-ar" ]]; then
        AR_BIN="$bin/llvm-ar"
    else
        AR_BIN="$bin/x86_64-w64-mingw32-ar"
    fi
    if [[ -x "$bin/llvm-windres" ]]; then
        WINDRES_BIN="$bin/llvm-windres"
    elif [[ -x "$bin/x86_64-w64-mingw32-windres" ]]; then
        WINDRES_BIN="$bin/x86_64-w64-mingw32-windres"
    else
        WINDRES_BIN=""
    fi
    if [[ -x "$bin/llvm-objdump" ]]; then
        OBJDUMP_BIN="$bin/llvm-objdump"
    elif [[ -x "$bin/x86_64-w64-mingw32-objdump" ]]; then
        OBJDUMP_BIN="$bin/x86_64-w64-mingw32-objdump"
    fi
    if [[ ! -x "$LINKER" ]]; then
        echo "工具链里没有 $LINKER" >&2
        exit 1
    fi
}

llvm_mingw_url() {
    case "$(uname -s)" in
        Darwin)
            printf '%s\n' "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW_TAG}/llvm-mingw-${LLVM_MINGW_TAG}-ucrt-macos-universal.tar.xz"
            ;;
        Linux)
            case "$(uname -m)" in
                x86_64)
                    printf '%s\n' "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW_TAG}/llvm-mingw-${LLVM_MINGW_TAG}-ucrt-ubuntu-22.04-x86_64.tar.xz"
                    ;;
                aarch64|arm64)
                    printf '%s\n' "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW_TAG}/llvm-mingw-${LLVM_MINGW_TAG}-ucrt-ubuntu-22.04-aarch64.tar.xz"
                    ;;
                *)
                    echo "暂不支持的 Linux 架构: $(uname -m)" >&2
                    exit 1
                    ;;
            esac
            ;;
        *)
            echo "暂不支持的系统: $(uname -s)" >&2
            exit 1
            ;;
    esac
}

install_portable_llvm_mingw() {
    local url archive dest top
    url="$(llvm_mingw_url)"
    archive="$CACHE/$(basename "$url")"
    dest="$CACHE/llvm-mingw"
    if [[ -x "$dest/bin/x86_64-w64-mingw32-clang" ]]; then
        use_gnullvm_toolchain "$dest/bin"
        return
    fi
    download_file "$url" "$archive"
    step "解压 LLVM MinGW"
    rm -rf "$CACHE/llvm-mingw-extract"
    mkdir -p "$CACHE/llvm-mingw-extract"
    tar -xJf "$archive" -C "$CACHE/llvm-mingw-extract"
    top="$(cd "$CACHE/llvm-mingw-extract" && echo */)"
    top="${top%/}"
    if [[ -z "$top" || ! -d "$CACHE/llvm-mingw-extract/$top/bin" ]]; then
        echo "LLVM MinGW 压缩包结构无法识别" >&2
        exit 1
    fi
    rm -rf "$dest"
    mv "$CACHE/llvm-mingw-extract/$top" "$dest"
    rm -rf "$CACHE/llvm-mingw-extract"
    use_gnullvm_toolchain "$dest/bin"
}

ensure_toolchain() {
    if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
        use_gnu_toolchain
        return
    fi
    if command -v x86_64-w64-mingw32-clang >/dev/null 2>&1; then
        use_gnullvm_toolchain "$(dirname "$(command -v x86_64-w64-mingw32-clang)")"
        return
    fi
    if brew_can_install; then
        step "安装 mingw-w64"
        if HOMEBREW_NO_AUTO_UPDATE=1 brew install mingw-w64 \
            && command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
            use_gnu_toolchain
            return
        fi
        echo "Homebrew 未能安装 mingw-w64，改为下载便携工具链。"
    fi
    install_portable_llvm_mingw
}

ensure_rust_target() {
    if ! command -v cargo >/dev/null 2>&1; then
        echo "未找到 cargo。请先安装 Rust：https://rustup.rs" >&2
        exit 1
    fi
    if ! command -v rustup >/dev/null 2>&1; then
        echo "未找到 rustup，无法添加 Windows 交叉编译目标" >&2
        exit 1
    fi
    if ! rustup target list --installed | grep -qx "$TARGET"; then
        step "添加 Rust 目标 $TARGET"
        rustup target add "$TARGET"
    fi
}

overlay_nsis_runtime() {
    local prefix="$1"
    local zip unpack data dir name
    if [[ -f "$prefix/Stubs/zlib-x86-unicode" && -d "$prefix/Include" && -d "$prefix/Contrib/Language files" ]]; then
        return
    fi
    zip="$CACHE/nsis-${NSIS_VERSION}.zip"
    unpack="$CACHE/nsis-${NSIS_VERSION}-win"
    download_file \
        "https://sourceforge.net/projects/nsis/files/NSIS%203/${NSIS_VERSION}/nsis-${NSIS_VERSION}.zip/download" \
        "$zip"
    if ! command -v unzip >/dev/null 2>&1; then
        echo "解压 NSIS 运行时需要 unzip" >&2
        exit 1
    fi
    step "准备 NSIS 安装器运行时"
    rm -rf "$unpack"
    mkdir -p "$unpack"
    unzip -q "$zip" -d "$unpack"
    data=""
    for dir in "$unpack"/*/Stubs "$unpack"/Stubs; do
        if [[ -d "$dir" ]]; then
            data="$(dirname "$dir")"
            break
        fi
    done
    if [[ -z "$data" ]]; then
        echo "NSIS 压缩包里没有 Stubs 目录" >&2
        exit 1
    fi
    mkdir -p "$prefix"
    for name in Stubs Include Plugins Contrib; do
        if [[ -d "$data/$name" ]]; then
            rm -rf "$prefix/$name"
            cp -R "$data/$name" "$prefix/$name"
        fi
    done
}

build_portable_nsis() {
    local archive src prefix sdk jobs
    archive="$CACHE/nsis-${NSIS_VERSION}-src.tar.bz2"
    src="$CACHE/nsis-${NSIS_VERSION}-src"
    prefix="$CACHE/nsis"
    if [[ ! -x "$prefix/Bin/makensis" && ! -x "$prefix/bin/makensis" && ! -x "$prefix/makensis" ]]; then
        if ! command -v python3 >/dev/null 2>&1; then
            echo "编译 NSIS 需要 python3" >&2
            exit 1
        fi
        download_file \
            "https://sourceforge.net/projects/nsis/files/NSIS%203/${NSIS_VERSION}/nsis-${NSIS_VERSION}-src.tar.bz2/download" \
            "$archive"
        if [[ ! -f "$src/SConstruct" ]]; then
            step "解压 NSIS 源码"
            rm -rf "$src"
            tar -xjf "$archive" -C "$CACHE"
        fi
        if [[ ! -f "$src/SConstruct" ]]; then
            echo "未找到 NSIS 源码目录: $src" >&2
            exit 1
        fi
        if [[ ! -d "$CACHE/py/SCons" ]]; then
            step "安装 scons"
            python3 -m pip install --disable-pip-version-check --target "$CACHE/py" scons
        fi
        step "编译 makensis"
        sdk=""
        if command -v xcrun >/dev/null 2>&1; then
            sdk="$(xcrun --show-sdk-path 2>/dev/null || true)"
        fi
        jobs="$(cpu_count)"
        (
            cd "$src"
            if [[ -n "$sdk" ]]; then
                PYTHONPATH="$CACHE/py" python3 -m SCons -j"$jobs" \
                    SKIPSTUBS=all \
                    SKIPPLUGINS=all \
                    SKIPUTILS=all \
                    SKIPDOC=all \
                    NSIS_CONFIG_CONST_DATA_PATH=no \
                    PREFIX="$prefix" \
                    APPEND_CPPPATH="$sdk/usr/include" \
                    APPEND_LIBPATH="$sdk/usr/lib" \
                    makensis
            else
                PYTHONPATH="$CACHE/py" python3 -m SCons -j"$jobs" \
                    SKIPSTUBS=all \
                    SKIPPLUGINS=all \
                    SKIPUTILS=all \
                    SKIPDOC=all \
                    NSIS_CONFIG_CONST_DATA_PATH=no \
                    PREFIX="$prefix" \
                    makensis
            fi
        )
        mkdir -p "$prefix/Bin"
        local built=""
        for built in "$src/build/urelease/makensis/makensis" "$src/build/release/makensis/makensis"; do
            if [[ -f "$built" && -x "$built" ]]; then
                cp "$built" "$prefix/Bin/makensis"
                chmod +x "$prefix/Bin/makensis"
                break
            fi
        done
    fi
    if [[ -x "$prefix/Bin/makensis" ]]; then
        MAKENSIS="$prefix/Bin/makensis"
    elif [[ -x "$prefix/bin/makensis" ]]; then
        MAKENSIS="$prefix/bin/makensis"
    elif [[ -x "$prefix/makensis" ]]; then
        MAKENSIS="$prefix/makensis"
    else
        echo "makensis 编译完成，但没有找到可执行文件" >&2
        exit 1
    fi
    overlay_nsis_runtime "$prefix"
}

ensure_makensis() {
    if command -v makensis >/dev/null 2>&1; then
        MAKENSIS="makensis"
        return
    fi
    if brew_can_install; then
        step "安装 nsis"
        if HOMEBREW_NO_AUTO_UPDATE=1 brew install nsis \
            && command -v makensis >/dev/null 2>&1; then
            MAKENSIS="makensis"
            return
        fi
        echo "Homebrew 未能安装 nsis，改为从源码编译 makensis。"
    fi
    build_portable_nsis
}

build_windows_exe() {
    local exe="$1"
    if [[ "$SKIP_BUILD" -eq 1 ]]; then
        step "跳过 cargo 编译 (--skip-build)"
    else
        step "cargo build --release --target $TARGET"
        if [[ "$TARGET" == "x86_64-pc-windows-gnullvm" ]]; then
            PATH="$PATH:$TOOLCHAIN_BIN" \
            CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER="$LINKER" \
            CC_x86_64_pc_windows_gnullvm="$CC_BIN" \
            CXX_x86_64_pc_windows_gnullvm="$CXX_BIN" \
            AR_x86_64_pc_windows_gnullvm="$AR_BIN" \
            WINDRES="${WINDRES_BIN}" \
            CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C target-feature=+crt-static" \
                cargo build --release --locked --target "$TARGET"
        else
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$LINKER" \
            CC_x86_64_pc_windows_gnu="$CC_BIN" \
            CXX_x86_64_pc_windows_gnu="$CXX_BIN" \
            AR_x86_64_pc_windows_gnu="$AR_BIN" \
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C target-feature=+crt-static" \
                cargo build --release --locked --target "$TARGET"
        fi
    fi

    if [[ ! -f "$exe" ]]; then
        echo "未找到可执行文件: $exe" >&2
        echo "请先成功完成 cargo build --release --target $TARGET" >&2
        exit 1
    fi
}

assert_static_runtime() {
    local exe="$1"
    if [[ -z "$OBJDUMP_BIN" || ! -x "$OBJDUMP_BIN" ]]; then
        if command -v "$OBJDUMP_BIN" >/dev/null 2>&1; then
            :
        else
            return
        fi
    fi
    local deps bad name
    deps="$("$OBJDUMP_BIN" -p "$exe" | sed -n 's/^[[:space:]]*DLL Name:[[:space:]]*//p')"
    bad=""
    for name in libgcc_s_seh-1.dll libstdc++-6.dll libwinpthread-1.dll libunwind.dll libc++.dll; do
        if printf '%s\n' "$deps" | grep -qi "^${name}$"; then
            bad="$bad $name"
        fi
    done
    if [[ -n "$bad" ]]; then
        echo "可执行文件仍依赖这些运行库:${bad}" >&2
        echo "干净的 Windows 上可能无法启动。请检查静态链接是否生效。" >&2
        exit 1
    fi
}

compile_nsis() {
    local version="$1"
    local version4="$2"
    local exe="$3"
    local dist="$ROOT/dist"
    local bytes size_kb

    mkdir -p "$dist"
    bytes="$(file_bytes "$exe")"
    size_kb=$(( (bytes + 1023) / 1024 ))

    step "使用 NSIS 编译安装包"
    # macOS 的 iconv 在 C 区域设置下无法把非 ASCII 的 wchar_t 转成 UTF-16。
    if [[ "$(locale charmap 2>/dev/null || true)" != "UTF-8" ]]; then
        export LC_ALL=en_US.UTF-8
        export LANG=en_US.UTF-8
    fi
    "$MAKENSIS" \
        -INPUTCHARSET UTF8 \
        -DVERSION="$version" \
        -DVERSION4="$version4" \
        -DSOURCE_DIR="$(dirname "$exe")" \
        -DOUT_DIR="$dist" \
        -DICON_FILE="$ROOT/assets/icon.ico" \
        -DREADME_FILE="$ROOT/README.md" \
        -DSIZE_KB="$size_kb" \
        "$ROOT/packaging/lan-share.nsi"
}

open_dist() {
    local dist="$1"
    case "$(uname -s)" in
        Darwin) open "$dist" ;;
        Linux)
            if command -v xdg-open >/dev/null 2>&1; then
                xdg-open "$dist" >/dev/null 2>&1 || true
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if command -v explorer.exe >/dev/null 2>&1; then
                explorer.exe "$(cygpath -w "$dist" 2>/dev/null || printf '%s' "$dist")" || true
            fi
            ;;
    esac
}

run_windows_packager() {
    local ps=""
    if command -v powershell.exe >/dev/null 2>&1; then
        ps="powershell.exe"
    elif command -v powershell >/dev/null 2>&1; then
        ps="powershell"
    else
        echo "未找到 PowerShell，无法调用 Inno Setup 打包脚本" >&2
        exit 1
    fi

    local -a args=()
    if [[ "$SKIP_BUILD" -eq 1 ]]; then
        args+=("-SkipBuild")
    fi
    if [[ "$OPEN_DIST" -eq 1 ]]; then
        args+=("-OpenDist")
    fi

    step "调用 Inno Setup 打包"
    if [[ ${#args[@]} -eq 0 ]]; then
        "$ps" -NoProfile -ExecutionPolicy Bypass -File "$ROOT/packaging/build.ps1"
    else
        "$ps" -NoProfile -ExecutionPolicy Bypass -File "$ROOT/packaging/build.ps1" "${args[@]}"
    fi
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

VERSION="$(read_version)"
printf '局域网文件分享 安装包制作\n'
printf '目录: %s\n' "$ROOT"
printf '版本: %s\n' "$VERSION"

ensure_icon

if is_windows; then
    run_windows_packager
    exit 0
fi

ensure_toolchain
ensure_rust_target
ensure_makensis

EXE="$ROOT/target/$TARGET/release/lan-share.exe"
build_windows_exe "$EXE"
assert_static_runtime "$EXE"

bytes="$(file_bytes "$EXE")"
printf '程序: %s (%.1f MB)\n' "$EXE" "$(awk -v n="$bytes" 'BEGIN { printf "%.1f", n/1048576 }')"

compile_nsis "$VERSION" "$(version_quad "$VERSION")" "$EXE"

installer="$ROOT/dist/LanShare_Setup_${VERSION}.exe"
printf '\n完成。\n'
if [[ -f "$installer" ]]; then
    ibytes="$(file_bytes "$installer")"
    printf '安装包: %s (%.1f MB)\n' "$installer" "$(awk -v n="$ibytes" 'BEGIN { printf "%.1f", n/1048576 }')"
else
    printf '输出目录: %s/dist\n' "$ROOT"
fi

if [[ "$OPEN_DIST" -eq 1 ]]; then
    open_dist "$ROOT/dist"
fi
