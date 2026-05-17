# DeskBridge

<p align="center">
  <img src="docs/assets/hero.svg" alt="DeskBridge 通过局域网让一台电脑的键盘、鼠标和剪切板控制另一台电脑。" width="100%">
</p>

<p align="center">
  <strong>更像原生应用的键盘、鼠标和剪切板共享工具。</strong>
</p>

<p align="center">
  <a href="README.md">English</a>
  ·
  <a href="https://github.com/Hades300/deskbridge/releases/latest">下载最新版</a>
  ·
  <a href="docs/MACOS.md">macOS</a>
  ·
  <a href="docs/WINDOWS.md">Windows</a>
</p>

## 这是做什么的？

DeskBridge 让你在同一张桌子上的多台电脑像一套工作站一样使用：键盘和鼠标留在主机上，鼠标从配置好的屏幕边缘滑出去后，输入就会切到另一台电脑；文本、图片和普通文件剪切板也可以跟随同步。

它受 Input Leap、Barrier、Synergy 这类工具启发，但不是它们的协议兼容实现，也没有复制它们的代码。DeskBridge 的重点是更稳定的重连、更清楚的诊断、更原生的 macOS/Windows 管理界面。

目前验证最多的场景是：

- Windows 作为 server，拥有真实键盘和鼠标。
- macOS 作为 client，被 Windows 远程控制。
- 根据屏幕相对位置跨边缘切换输入。
- Windows 睡眠、重启、网络恢复后，Mac client 自动重连。
- 支持文本、图片和普通文件剪切板同步。

## 截图

<p align="center">
  <img src="docs/assets/screenshots/deskbridge-macos.png" alt="DeskBridge macOS 客户端窗口" width="86%">
</p>

## 亮点

- macOS 状态栏应用 + Windows WPF 管理面板。
- Rust 核心 daemon 和协议层，包含 heartbeat、连接过期检测和自动重连。
- 支持屏幕布局配置，能处理两端屏幕尺寸不同的跨边缘映射。
- 剪切板同步：文本、图片、普通文件。
- 远程诊断：屏幕信息、对端版本、server/client 日志、route probe、capture probe、性能指标。
- 支持滚轮方向和远端滚轮速度调节。
- 在跨回本机或断连时主动释放远端残留按键，降低 Alt/Option 等修饰键卡住的概率。

## 下载

最新版：

[github.com/Hades300/deskbridge/releases/latest](https://github.com/Hades300/deskbridge/releases/latest)

安装包：

- `DeskBridge-macos.dmg`
- `DeskBridge-macos.zip`
- `DeskBridge-windows-x64.zip`
- `DeskBridge-windows-arm64.zip`
- `DeskBridge-linux-x64.tar.gz`

Windows 压缩包里有两个程序：优先打开 `DeskBridge.Admin.exe`。`deskbridge.exe` 是命令行 daemon，正常使用时由管理面板启动。

## 快速开始：Windows 控制 Mac

1. 在 Mac 上安装 DeskBridge。
2. 在 Windows 上下载并解压 `DeskBridge-windows-x64.zip`。
3. Windows 打开 `DeskBridge.Admin.exe`。
4. 保持监听地址类似 `0.0.0.0:24800`，点击 **Start Server**。
5. Windows 防火墙提示时允许 Private networks。
6. Mac 打开 DeskBridge，选择 **Client**。
7. Server 填 Windows 的局域网地址，例如 `192.168.2.5:24800`。
8. Mac screen name 填 `mac`，Windows peer name 填 `windows`。
9. 按提示给 macOS Accessibility 权限。
10. 从 Windows 屏幕配置好的边缘滑出鼠标，进入 Mac。

如果 macOS 一直提示需要辅助功能权限，要确认授权的是 `DeskBridge.app/Contents/Helpers/DeskBridgeHelper.app` 里的 helper 进程，而不是旧版本 app 或某个终端进程。

## 常用界面能力

- **Mode**：选择 Server 或 Client。
- **Connection**：配置监听地址、连接地址、本机名和对端名。
- **Layout**：在 server 上拖动对端屏幕位置，匹配真实桌面摆放。
- **Clipboard**：控制文本、图片和文件同步。
- **Recovery**：开启自动重连，应对睡眠、Wi-Fi 切换和 server 重启。
- **Diagnostics**：遇到无法跨屏、卡顿、断连时跑诊断，而不是复制一大段日志。

## 命令行

创建配置：

```bash
deskbridge init-config --path deskbridge.json
```

运行 server：

```bash
deskbridge server --listen 0.0.0.0:24800 --name windows --allow mac --capture
deskbridge server --config examples/deskbridge.json --capture
```

运行 client：

```bash
deskbridge client --server 192.168.2.5:24800 --name mac --reconnect
deskbridge client --config examples/deskbridge.json
```

诊断：

```bash
deskbridge diag --server 192.168.2.5:24800 --name mac
deskbridge debug --server 192.168.2.5:24800 --name mac route-status
deskbridge debug --server 192.168.2.5:24800 --name mac perf
```

## 从源码构建

需要：

- Rust stable
- macOS app 需要 SwiftPM 和 Xcode Command Line Tools
- Windows 管理面板需要 .NET 8 SDK

```bash
source "$HOME/.cargo/env"
cargo build --workspace
cargo test --workspace
./scripts/verify-local.sh
```

本地打包 macOS app：

```bash
./scripts/package-macos-app.sh
open build/DeskBridge.app
```

## 当前边界

已经可以日常试用和迭代：

- Windows server 到 macOS client 的键鼠共享。
- 自动重连和连接过期恢复。
- 文本、图片、普通文件剪切板同步。
- server 侧屏幕布局编辑和诊断。
- GitHub Actions 自动打 macOS/Windows/Linux 包。

仍在打磨：

- 目录剪切板传输和大文件流式传输。
- 更完整的签名安装器和 macOS notarization。
- 更多 Windows/macOS/Linux 多显示器拓扑验证。

## License

MIT.
