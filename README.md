# 局域网文件分享

Windows 桌面小工具：把本机文件夹分享到同一局域网，手机或其他设备可用浏览器下载，也支持扫码访问。

## 功能

- 选择一个或多个文件夹进行分享
- 启动后自动放行 Windows 防火墙（TCP 端口），停止时移除本次添加的规则
- 显示局域网访问地址与二维码（不含 `127.0.0.1`）
- 浏览器端可浏览目录、下载文件（支持断点续传）
- 配置保存在程序同目录的 `lan-share.json`

## 环境要求

- Windows 10 / 11
- [Rust](https://www.rust-lang.org/)（建议稳定版）
- 启动服务时可能弹出 UAC，需允许管理员权限以配置防火墙

## 构建与运行

```bash
# 开发运行
cargo run

# 发布构建
cargo build --release
```

发布产物：`target/release/lan-share.exe`

## 一键打包

在仓库根目录执行，生成 `dist/LanShare_Setup_<版本>.exe`。

### macOS / Linux

```bash
./pack.sh
./pack.sh --skip-build
./pack.sh --open
```

脚本会交叉编译 Windows 程序，再用 NSIS 打出安装包。本机已有 MinGW 和 NSIS 时直接使用；否则在 Homebrew 可用时自动安装。Homebrew 不可用时，会把 LLVM MinGW 和 NSIS 准备到 `~/.cache/lan-share-pack`（首次需要联网）。可执行文件静态链接运行库，安装包里只需放入 `lan-share.exe`。

### Windows

需要已安装 [Inno Setup 6](https://jrsoftware.org/isinfo.php)。

```bat
pack.bat
```

或：

```powershell
.\packaging\build.ps1
.\packaging\build.ps1 -SkipBuild
.\packaging\build.ps1 -OpenDist
```

在 Windows 的 Git Bash 里执行 `./pack.sh` 时，会转去调用上述 Inno Setup 流程。

### macOS 安装盘

```bash
./pack-mac.sh
./pack-mac.sh --skip-build
./pack-mac.sh --open
```

在 Mac 上编译原生程序，生成 `dist/LanShare_<版本>.dmg`。打开后把「局域网文件分享」拖到「应用程序」。macOS 上的配置写在 `~/Library/Application Support/局域网文件分享/lan-share.json`，不会自动修改系统防火墙。

图标资源在 `assets/icon.ico`（可用 `python assets/gen_icon.py` 重新生成）。

## 使用说明

1. 打开程序，点右上角齿轮进入设置
2. 选择要分享的文件夹，必要时修改端口（默认 `8000`）
3. 返回二维码页，点击 **开始**
4. 同一局域网内的设备用浏览器打开显示的地址，或扫描二维码
5. 需要结束分享时，在二维码页点击右上角状态中的 **停止**（设置页也有停止按钮）

未选择分享路径时，不会使用当前目录作为默认分享内容。

## 配置

配置文件路径：与可执行文件同目录下的 `lan-share.json`

示例：

```json
{
  "port": 8000,
  "paths": [
    "D:\\Photos",
    "D:\\Docs"
  ]
}
```

## 防火墙说明

- 规则名格式：`LanShare-{端口}`
- 仅当本次启动新建了规则时，停止服务会删除该规则
- 若启动前规则已存在，则停止时不会删除

## 许可证

仅供个人/内网使用，请勿对外公开未授权的文件内容。
