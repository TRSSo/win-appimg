# WinAppImg

> [English](./README_en.md) | 简体中文

[![License: LGPL-3.0](https://img.shields.io/badge/%E8%AE%B8%E5%8F%AF%E8%AF%81-LGPL--3.0-blue.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/%E7%B3%BB%E7%BB%9F-Windows-blue.svg)
![Edition](https://img.shields.io/badge/rust-2024-orange.svg)
[![Release](https://img.shields.io/github/release/TRSSo/win-appimg)](https://github.com/TRSSo/win-appimg/releases/latest)

> 一个创建 Windows 应用程序镜像的工具。使用 WinFsp + DwarFS，原理类似于 AppImage。

WinAppImg 把一个完整的应用程序目录（或其 DwarFS 镜像）打包成**单个 `.exe` 文件**。用户可直接运行，无需安装、无需解压。

---

## 目录

- [项目简介](#项目简介)
- [主要功能特性](#主要功能特性)
- [技术栈](#技术栈)
- [环境要求](#环境要求)
- [使用说明](#使用说明)
- [配置项说明](#配置项说明)
- [工作原理](#工作原理)
- [许可证](#许可证)
- [贡献指南](./CONTRIBUTING.md)

---

## 项目简介

在 Linux 上，[AppImage](https://appimage.org/) 让我们能够“一个文件就是一个应用”。WinAppImg 把同样的体验带到 Windows：

- **打包者**：把一个应用的整个目录打成单个 exe，可以自定义图标、版本信息、UAC 清单。
- **使用者**：直接运行即可，首次运行时需安装运行时依赖（约 10 MiB）。

与自解压包不同，WinAppImg **不会**把文件释放到磁盘：

```
传统自解压包：exe ──解压──> %TEMP%\xxx\（所有文件）──运行──> app.exe
WinAppImg：   exe ──挂载──> %TEMP%\虚拟盘（按需读取，高度压缩）──运行──> app.exe
```

应用文件始终以压缩形态留在 exe 内部，由 DwarFS 文件系统按需解压，冷启动只读取真正被访问到的块。

### 产物构成

| 文件 | 作用 |
| :--- | :--- |
| `win-appimg-cli` | **CLI 打包工具**：开发者用它把应用目录构建成应用镜像 |
| `win-appimg-gui` | **GUI 打包工具**：（开发中）用户自己打包归档应用程序 |
| `win-appimg-depend` | **依赖管理器**：可选的 GUI 工具，用于安装/卸载运行时依赖 |

---

## 主要功能特性

### CLI 打包工具

- **目录直出镜像**：直接传入应用目录，内置打包器递归构建 DwarFS 镜像，也可用 `--use-mkdwarfs` 调用外部 `mkdwarfs`
- **复用已有镜像**：如有自定义打包要求，传入 `.dwarfs` 文件时直接复用，跳过打包步骤（注意若传入文件使用非 ZSTD 压缩，将无法使用兼容模式）
- **高压缩比**：ZSTD 压缩，等级 1–22 可调（`--compress-level`）
- **PE 资源定制**：
  - 从启动程序复制版本信息（`--copy-info`）与图标（`--copy-icon`）
  - 手动设置版本信息字段（`--set-info KEY=VALUE`）与图标（`--set-icon`）
  - 请求管理员权限（`--req-admin`）
  - 显示控制台（`--show-console`）
- **完整的启动配置**：支持启动外部程序（`--external`）、传入启动参数、设置工作目录（`--workdir`）
- **参数占位符**：启动参数中的 `{}` 在运行时替换为应用根目录，可用于 `cmd /c start "{}\start.cmd"` 这类场景

### 启动器

- **按需挂载**：通过 WinFsp + DwarFS 挂载应用镜像，退出后自动卸载
- **可写层**（实验性）：不建议使用，建议修改程序行为，不要写入程序目录
  - `Save` — 保存在 exe 同目录的 `.data` 文件夹，持久保留
  - `Temp` — 保存在临时目录，退出即弃
  - `Memory` — 保存在 MemFS 内存文件系统
- **一键安装依赖**：首次运行自动申请管理员权限一键安装
- **兼容模式**：无法安装启动运行时依赖、无法运行程序时，可退化为解压到临时目录后启动
- **启动修复**：启动失败时弹出修复向导，支持重装依赖、切换兼容模式
- **进程组管理**：应用与依赖进程统一纳入 Windows Job Object，主进程退出时子进程被可靠回收
- **启动成功判定**：可配置“启动后 N 秒内退出视为启动失败”
- **中文图形界面**：TaskDialog 对话框，无控制台窗口闪烁

---

## 技术栈

### 语言与构建

| 项目 | 说明 |
| :--- | :--- |
| 语言 | Rust，Edition 2024（需 Rust 1.85+） |
| 构建系统 | Cargo，含 `build.rs` 构建脚本 |

### 核心依赖

| 依赖 | 用途 |
| :--- | :--- |
| [`dwarfs`](https://crates.io/crates/dwarfs) | DwarFS 镜像读取与解压 |
| [`dwarfs-enc`](https://crates.io/crates/dwarfs-enc) | DwarFS 镜像打包（CDC 分块 + 去重 + ZSTD） |
| [`editpe`](https://crates.io/crates/editpe) | PE 文件解析与改写（资源、子系统、节表） |
| [`zerompk`](https://crates.io/crates/zerompk) | MessagePack 序列化（内嵌配置） |
| [`windows-sys`](https://crates.io/crates/windows-sys) | Win32 API 绑定（Job Objects、IOCP、Shell、Registry） |

### 运行时依赖

| 组件 | 版本要求 | 来源 |
| :--- | :--- | :--- |
| [WinFsp](https://github.com/winfsp/winfsp) | 2.2+ | 由安装脚本静默安装 MSI 包 |
| [DwarFS](https://github.com/mhx/dwarfs) | ≥ 0.15.7 | 负责挂载镜像 |
| [OverlayFS](https://github.com/TRSSo/WinFsp-OverlayFS) | ≥ 0.0.1 | 仅使用可写层时需要 |

> 安装后约占用 10 MiB 磁盘空间。上述版本要求会在每次启动时自动校验，不满足时提示升级。无法安装时会退化为解压到临时目录后启动。

---

## 环境要求

### 构建环境

- **Windows** 支持全部特性
- **非 Windows** 无法构建特性 launcher、depend
- **Rust 1.85 或更高版本**（Edition 2024）
- **curl** 下载运行时依赖
- **7z** 解压运行时依赖

### 打包环境

- Windows、Linux、macOS 等 Rust 支持的系统
- `mkdwarfs` 使用 `--use-mkdwarfs` 或有自定义打包要求时（可选）

### 运行环境

- Windows 7 及以上
- 首次使用时需要管理员权限安装运行时依赖（会弹 UAC）
- 使用兼容模式时无特殊权限要求

---

## 使用说明

### 方式一：下载预编译产物（推荐）

从 [Releases](https://github.com/TRSSo/win-appimg/releases/latest) 页面下载

### 方式二：从源码构建

```bash
# 1. 克隆仓库
git clone https://github.com/TRSSo/win-appimg.git
cd win-appimg

# 2. Windows 构建全部组件
cargo build -r --all-features
# 非 Windows 无法构建全部组件
cargo build -r

# 3. Windows 运行构建后处理
target/release/post_build.exe
# 非 Windows 需要带上 launcher.exe
target/release/post_build path/to/launcher.exe
```

### CLI 打包工具

基本格式：

```bash
win-appimg-cli <SOURCE> <OUTPUT> [OPTIONS] -- <启动命令> [启动参数...]
```

- `<SOURCE>` — 应用目录，或已构建好的 `.dwarfs` 镜像文件
- `<OUTPUT>` — 输出的 exe 路径，扩展名不是 `.exe` 时会自动补上
- `--` 之后的内容是**镜像内应用的启动命令**，会原样保存并在运行时执行
- 启动参数里的 `{}` 会在运行时替换为**应用根目录**，详见[参数占位符](#参数占位符)

#### 示例

> Windows 上操作时，需要替换续行符 `\`：cmd `^`、PowerShell `` ` ``。

```bash
# 打包整个目录，启动其中的 app.exe
win-appimg-cli myapp dist/MyApp -- app.exe

# 启动命令带固定参数（运行时传入的参数会追加在其后）
win-appimg-cli myapp dist/MyApp -- app.exe --config conf.ini

# 指定工作目录为程序目录
win-appimg-cli myapp dist/MyApp -w . -- app.exe

# 指定工作目录为程序目录下指定目录
win-appimg-cli myapp dist/MyApp -w data -- app.exe

# 复用已有的 DwarFS 镜像，跳过打包
win-appimg-cli app.dwarfs dist/MyApp -- app.exe

# 降低压缩等级以加快打包（默认 22，追求速度可用 3~7）
win-appimg-cli myapp dist/MyApp -c 7 -- app.exe

# 定制 PE 资源
win-appimg-cli myapp dist/MyApp \
  --set-info FileVersion=1.2.3.0 \
  --set-info ProductName=MyApp \
  --set-info FileDescription="我的应用" \
  --set-icon myapp.ico \
  -- app.exe

# 从启动程序复制资源
win-appimg-cli myapp dist/MyApp --copy-info --copy-icon -- app.exe

# 需要请求管理员权限的程序
win-appimg-cli myapp dist/MyApp --req-admin -- app.exe

# 打包命令行程序
win-appimg-cli myapp dist/MyApp --show-console -- app.exe

# 用 {} 引用应用根目录，交给系统命令处理器启动脚本
# cmd 不在镜像内，因此必须加 -e
win-appimg-cli myapp dist/MyApp -e -- cmd /c start "{}\start.cmd"

# 可写层保存在 exe 同目录的 MyApp.data 文件夹，退出后保留
win-appimg-cli myapp dist/MyApp --write -- app.exe

# 可写层放在内存盘，退出即弃
win-appimg-cli myapp dist/MyApp --write-memory -- app.exe

# 不内嵌依赖（减小约 3 MiB，运行时按需下载）
win-appimg-cli myapp dist/MyApp --no-depend -- app.exe
```

### 运行打包产物

普通程序直接运行即可，命令行程序也可传入参数，追加在打包时设定的参数之后：

```bash
# 构建时指定：app.exe --config conf.ini
MyApp.exe --verbose
# 实际执行：app.exe --config conf.ini --verbose
```

### 管理依赖

运行 `win-appimg-depend`，在弹出的窗口中选择 `安装` 或 `卸载`

```
当前状态：已安装
──────────────────
  [安装]  [卸载]
```

> 安装与卸载日志保存在 `<临时目录>\install.log`，失败时会提示是否查看。

---

## 配置项说明

### Cargo Features

| Feature | 默认 | 说明 |
| :--- | :--- | :--- |
| `cli` | ✅ | 构建打包命令行工具 `cli` |
| `depend` | ❌ | 构建依赖管理器 `depend`，并让 `post_build` 为其追加依赖包 |
| `launcher` | ❌ | 构建应用镜像入口 `launcher`，`post_build` 会把它内嵌进 `cli` |

### 命令行参数

#### 位置参数

| 参数 | 说明 |
| :--- | :--- |
| `<SOURCE>` | 源文件路径。目录则递归打包；文件则视为已构建好的 DwarFS 镜像 |
| `<OUTPUT>` | 输出文件路径。扩展名不是 `.exe` 时自动追加 |
| `[COMMAND]...` | `--` 之后的启动命令与固定参数，运行时传入的参数追加在其后。固定参数中的 `{}` 会在运行时替换为应用根目录 |

#### 选项

| 选项 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `-w, --workdir <DIR>` | — | 应用工作目录，相对路径相对镜像挂载点解析 |
| `-c, --compress-level <1-22>` | `22` | ZSTD 压缩等级。数值越大体积越小、打包越慢 |
| `-e, --external` | 关 | 启动外部程序，不使用镜像内的程序 |
| `--no-depend` | 关 | 不内嵌 WinFsp 依赖包，运行时按需引导下载 |
| `--show-console` | 关 | 显示控制台窗口（把 PE 子系统设为 CUI） |
| `--req-admin` | 关 | 把清单中的 `asInvoker` 替换为 `requireAdministrator` |
| `--copy-info` | 关 | 从启动程序复制版本信息资源 |
| `--copy-icon` | 关 | 从启动程序复制图标资源 |
| `--set-info <KEY=VALUE>` | — | 设置版本信息字段，可重复传入 |
| `--set-icon <FILE>` | — | 设置图标文件 |
| `--set-manifest <FILE>` | — | 替换 UAC 清单文件 |
| `--use-mkdwarfs` | 关 | 调用外部 `mkdwarfs` 构建镜像，而非内置打包器 |
| `--started-time <SECOND>` | `0` | 启动后在该秒数内退出则视为启动失败，`0` 表示不检查 |
| `--write` | 关 | 可写层保存到 exe 同目录的 `.data` 文件夹（实验性） |
| `--write-temp` | 关 | 可写层保存到临时目录，退出即弃（实验性） |
| `--write-memory` | 关 | 可写层保存到内存文件系统（实验性） |
| `-h, --help` | — | 显示帮助（`-h` 为简短版） |
| `-V, --version` | — | 显示版本 |

#### 参数占位符

镜像挂载点是运行时随机生成的，打包时无法预知。启动参数里可以用 `{}` 引用它：

| 参数写法 | 运行时结果 |
| :--- | :--- |
| `{}` | 应用根目录（挂载点；兼容模式下为解压目录） |
| `{{}}` | 字面量 `{}` |
| `{`、`{a}` 等 | 原样保留 |

```bash
# 打包时写入
-- cmd /c start "{}\start.cmd"
# 运行时实际执行
cmd /c start "C:\Users\me\AppData\Local\Temp\Ab3xY9\start.cmd"
```

规则说明：

- 只对 `--` 之后的**固定参数**生效，运行时由使用者追加的参数不会被替换
- `<启动命令>` 本身与 `--workdir` 无需占位符：两者的相对路径已相对镜像挂载点解析
- 替换后仍是**一个**参数，路径含空格也不会被拆开
- 启动镜像外的程序（如 `cmd`）时需要加 `-e`，否则打包阶段会因找不到启动命令而失败

#### `--set-info` 支持的键

可设置任意 Windows 版本信息字符串（如 `ProductName`、`CompanyName`、`FileDescription`、`LegalCopyright`）。

其中 `FileVersion` 与 `ProductVersion` 除了写入字符串表，还会被解析为 PE 的二进制版本号：

```bash
--set-info FileVersion=1.2.3.4      # -> major = 1<<16 | 2, minor = 3<<16 | 4
```

解析规则：按 `.` 分割，每段按 `u16` 解析，非法段按 `0` 处理，最多使用前 4 段。

### 可写层模式对比

| 模式 | 参数 | 存储位置 | 退出后 | 典型场景 |
| :--- | :--- | :--- | :--- | :--- |
| 只读 | （默认） | — | — | 不需要写入程序目录的绝大多数应用 |
| `Save` | `--write` | exe 同目录 `<name>.data` | 保留 | 需要保存写入程序目录的文件 |
| `Temp` | `--write-temp` | 临时目录 | 丢弃 | 运行时要写临时文件，但无需保留 |
| `Memory` | `--write-memory` | MemFS 内存盘 | 丢弃 | 写入频繁，且不希望占用磁盘 |

> 可写层目前为**实验性**功能，因为依赖的 OverlayFS 是实验性版本。建议修改程序行为，不要写入程序目录。

---

## 工作原理

### 产物布局

```
┌───────────────────────────────────┐
│ launcher.exe（PE 映像）           │  启动器本体
│   └─ .payload 段：offset + length │  指向 LauncherPayload
├───────────────────────────────────┤
│ 运行时依赖（可选）                │
├───────────────────────────────────┤
│ 应用 DwarFS 镜像                  │
├───────────────────────────────────┤
│ LauncherPayload（MessagePack）    │  启动配置 + 各段偏移/长度/摘要
└───────────────────────────────────┘
```

> `.payload` 段在源码中是一个 16 字节占位符 `PAYLOAD\u{F05F}OFFSET`，构建期由 `post_build` 与 `cli` 把真实的偏移与长度写回。启动器运行时通过 `read_volatile` 读取该结构，定位并反序列化末尾的配置。

### 启动流程

```
用户启动 MyApp.exe
    │
    ├─ 读 .payload 段 → 反序列化 LauncherPayload
    │
    ├─ 检查运行时依赖
    │      └─ 缺失 → 弹窗 → 解压内嵌依赖 → UAC 提权静默安装
    │                          └─ 未内嵌 → 弹窗提示下载安装
    │
    ├─ 挂载镜像到临时目录（轮询等待挂载点出现，约 100s 超时）
    │
    ├─ 启动应用进程（继承控制台或隐藏窗口）
    │      └─ 若配置了 started_time，在该秒内退出则视为启动失败
    │
    └─ 等待任一进程组退出
           ├─ 应用退出 → 停止依赖、清理临时文件、返回其退出码
           └─ 启动失败、依赖退出 → 报错并进入修复向导
```

> 所有子进程都被分配到独立的 Windows Job Object，并关联到同一个 IOCP。Job 设置为 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，因此即使启动器被强制结束，应用与挂载进程也会被系统回收，不会残留挂载点。

---

## 许可证

本项目基于 **GNU Lesser General Public License v3.0 (LGPL-3.0)** 授权，详见 [LICENSE](./LICENSE)。

### 第三方组件

打包产物在运行时会用到以下组件，它们各自遵循自己的许可证：

| 组件 | 许可证 |
| :--- | :--- |
| [WinFsp](https://github.com/winfsp/winfsp) | GPL-3.0（含 FLOSS 例外） |
| [DwarFS](https://github.com/mhx/dwarfs) | MIT |
| [WinFsp-OverlayFS](https://github.com/TRSSo/WinFsp-OverlayFS) | LGPL-3.0 |

> 分发基于 WinAppImg 打包的应用时，请注意上述组件的许可要求。WinAppImg 本身不随产物分发 WinFsp 之外的 GPL 代码。
