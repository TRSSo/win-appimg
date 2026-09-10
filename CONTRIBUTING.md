# 贡献指南

> [English](./CONTRIBUTING_en.md) | 简体中文

感谢你愿意为 WinAppImg 做出贡献！

本项目是一个 Rust 编写的 Windows 应用镜像打包工具，涉及 PE 文件结构、Windows 文件系统驱动（WinFsp）、进程与 Job Object 管理等偏底层的知识。无论你是想修个错别字、补充测试，还是实现新特性，都欢迎参与。

本文档说明**开发环境搭建、代码规范、提交与 PR 流程**。如果你只想了解如何使用本工具，请参阅 [README.md](./README.md)。

## 目录

- [我可以怎么参与](#我可以怎么参与)
- [开发环境](#开发环境)
- [构建与测试](#构建与测试)
- [代码规范](#代码规范)
- [需要特别留意的改动](#需要特别留意的改动)
- [提交信息规范](#提交信息规范)
- [拉取请求流程](#拉取请求流程)
- [发布流程（维护者）](#发布流程维护者)
- [许可证](#许可证)

---

## 我可以怎么参与

本仓库提供了 GitHub Issue 模板，位于 [`.github/ISSUE_TEMPLATE/`](.github/ISSUE_TEMPLATE/)：

| 模板 | 用途 |
| :--- | :--- |
| `bug_report.md` | **缺陷报告** —— 打包 / 启动 / 依赖 / 可写层 / 构建异常 |
| `feature_request.md` | **功能建议** —— 新特性或改进 |
| `question.md` | **使用问题** —— 使用上的疑问 |

点击仓库的 [New Issue](https://github.com/TRSSo/win-appimg/issues/new/choose) 即可选择。模板已包含需要填写的环境信息表与确认项，请尽量填写完整。

### 报告 Bug

提交前请先搜索是否已有相同问题。缺陷报告需要的关键信息：

| 项目 | 说明 |
| :--- | :--- |
| 环境 | Windows 版本与补丁号、CPU 架构（x64 / x86 / ARM64） |
| 版本 | 本项目的版本号，尽量使用最新版本报告 |
| 组件版本 | WinFsp、DwarFS 的版本号，以及其他有用到组件的版本号 |
| 复现步骤 | 完整的命令行，以及输入目录的结构 |
| 实际输出 | 打包或运行时的完整输出，可用 `--show-console` 打开运行日志 |
| 预期行为 | 你认为正确的结果是什么 |

> 涉及打包失败的，请附上 `cli` 的完整输出；涉及启动失败的，请附上弹窗中的错误信息；涉及依赖安装的，请附上 `install.log`。

### 报告安全问题

安全漏洞请**不要**提交公开 Issue。请使用仓库 Security 页的「Report a vulnerability」。

### 提出新功能

请先开一个 Issue 描述需求与使用场景，避免重复劳动或方向偏差。以下内容特别欢迎讨论：

- 打包体积与启动速度的进一步优化
- 更完善的可写层方案
- 启动失败的修复逻辑
- 非 Windows 平台的构建支持

### 其他

- 改进文档、补充示例
- 补充单元测试（尤其是边界条件）
- 翻译与文案润色

---

## 开发环境

### 必需

| 工具 | 版本 | 说明 |
| :--- | :--- | :--- |
| Rust | **1.85+** | 项目使用 Edition 2024 |
| Windows | 7+ | 见下方「关于非 Windows 系统」 |

### 可选

仅在需要**重新下载并打包依赖**时才用到（即运行 `post_build` 且本地无缓存时）：

| 工具 | 用途 |
| :--- | :--- |
| `curl` | 下载运行时依赖 |
| `7z` | 解压运行时依赖 |
| `mkdwarfs` | 仅在调试 `--use-mkdwarfs` 路径时需要 |

### 关于非 Windows 系统

[win 模块](./src/win/)包含只能在 Windows 上编译的部分。

有些 feature 只能在 Windows 上编译，例如 `launcher` 和 `depend`。

| 系统 | 推荐命令 | 说明 |
| :--- | :--- | :--- |
| Windows | `cargo clippy --all-targets --all-features`<br>`cargo test --all-features` | 可构建与测试全部目标 |
| Linux / macOS | `cargo clippy --all-targets`<br>`cargo test` | **只能使用默认 feature** |

**请务必保证你的改动在非 Windows 上也能通过 `cargo check`**（默认 feature）。

通用模块（如 `dwarfs.rs`、`payload.rs`）里需要区分平台时，请沿用 `dwarfs.rs::create_symlink` 的三分支写法：

```rust
#[cfg(unix)]
fn create_symlink(..) { .. }

#[cfg(windows)]
fn create_symlink(..) { .. }

// 兜底分支，保证其他平台也能编译
#[cfg(not(any(unix, windows)))]
fn create_symlink(..) { .. }
```

---

## 构建与测试

### Feature 说明

| Feature | 默认 | 构建的二进制 |
| :--- | :--- | :--- |
| `cli` | ✅ | 打包命令行工具 |
| `depend` | ❌ | 依赖管理器 GUI |
| `launcher` | ❌ | 应用镜像入口，会被内嵌进 `cli` |

```bash
# 日常开发（仅 cli，编译最快）
cargo build

# 全量检查：所有 feature、所有目标（含测试代码）
cargo clippy --all-targets --all-features

# 构建完整发布产物
cargo build -r --all-features
```

### 完整产物构建

仅 `cargo build` 得到的程序内部还没有依赖包与启动器，必须再跑一次构建后处理：

```bash
target/release/post_build.exe
# 非 Windows 需带上 launcher.exe
target/release/post_build path/to/launcher.exe
```

它会下载并校验依赖包、打包成 `depend.dwarfs`，然后把依赖与 `launcher.exe` 追加进产物。

> 依赖包会缓存在构建目录，重复执行时通过 SHA256 校验后直接复用。
>
> 注意：[res 目录](./res/)缺失时 `build.rs` 会跳过图标与清单嵌入，只用于作为其他项目依赖时，构建产物必须包含清单。

### 测试说明

项目目前的单元测试，全部位于各源文件末尾的 `#[cfg(test)] mod tests` 中。新增测试时请遵循：

- **位置**：与被测代码同文件的 `mod tests`，使用 `use super::*;`
- **隔离**：用 `tempfile` 创建临时目录 / 文件，不要依赖真实路径，也不要在仓库里留残留文件
- **命名**：`被测函数_场景_预期结果`，例如 `segment_file_open_rejects_offset_beyond_size`
- **注释**：在断言上方用 `//` 说明「为什么期望这个值」，尤其是边界与反直觉的分支
- **平台**：只能在 Windows 上跑的测试标注 `#[cfg(windows)]`（[win 模块](./src/win/)内不需要）；跨 unix / windows 的用 `#[cfg(any(unix, windows))]`

---

## 代码规范

### 格式化与 Lint

- 提交前运行 `cargo fmt`，配置已由 `rustfmt.toml` 固定
- `cargo clippy --all-targets --all-features` 必须**零警告**。若确实需要豁免，请加 `#[allow(..)]` 并在注释里说明理由

### 注释

- 公开方法、公开类型、关键私有方法都要有 `///` 文档注释
- 文档注释需说明**功能、参数、返回值**，并列出错误分支对应的 `ErrorKind`
- 复杂逻辑辅以 `//` 行注释解释「为什么这么做」，而不是复述代码

```rust
/// 打开 `path`，并把它从 `offset` 开始、长度为 `length` 的区间作为独立文件
///
/// `offset` 超过文件大小，或 `length` 超出 `[offset, 文件大小)` 时返回 `ErrorKind::InvalidInput`。
pub fn open<P: AsRef<Path>>(path: P, offset: u64, length: u64) -> Result<Self> {
```

### 命名与语言

- 标识符使用英文
- 文档注释、行内注释使用**中文**
- GUI 程序的输出仅在 `#[cfg(debug_assertions)]` 下打印

### 国际化

用户可见文案（错误信息、对话框、命令行输出）一律通过 `m!` 宏输出，中英双语集中放在 [lang 目录](./lang/) 的 `*.yml` 里，键为 `[英文, 中文]`：

`build.rs` 会为每个 `*.yml` 在 `OUT_DIR/i18n/` 下生成同名的 `.rs`（如 `lang/lib.yml` -> `i18n/src/lib.rs`），由源文件顶层的 `include_lang!()` 引入。带 `{}` 占位的文案会生成格式化宏，**中英两侧的占位符必须一致**。
新增文案后记得在文件开头添加 `include_lang!()`，程序入口调用 `init_lang()`。

文案里**无法直接写字面量 `{}`**：按 `format!` 的格式化规则，传入 `{{}}` 代表字面量 `{}`，并使用 `m!(msg,)` 代表无参数 `format!`。

### 错误处理

- 统一使用 `std::io::Result<T>` 与 `std::io::Error`，不引入自定义 error 类型
- 使用语义准确的 `ErrorKind`：`NotFound`、`InvalidInput`、`InvalidData`、`UnexpectedEof`、`Unsupported`、`PermissionDenied`、`Interrupted` 等
- 包装第三方错误时带上国际化上下文：

  ```rust
  .map_err(|e| Error::other(m!(AddFileError, e)))?;
  ```

- 整数运算注意溢出：先比较再做减法，或使用 `saturating_sub` / `checked_sub`

### 平台相关代码

- `src/win/` 是 Windows 专属模块，其中的代码无需考虑跨平台
- 通用模块里新增平台分支时，必须提供 `#[cfg(not(any(unix, windows)))]` 兜底
- 新增 Windows API 调用前，确认 `Cargo.toml` 中 `windows-sys` 已启用对应 feature（如 `Win32_System_JobObjects`）

---

## 需要特别留意的改动

### 1. MessagePack 结构体字段

`PayloadData`、`Payload`、`LauncherPayload` 等会被序列化进产物末尾。

**增删或重排字段会破坏旧产物的反序列化**，非 Windows 平台测试时，需确保使用 Windows 构建出最新 launcher.exe。

### 2. `Cargo.toml` 的 `include`

新增需要随 crate 一起发布的**非 `.rs` 文件**时，必须同步更新这一项，否则 `cargo package` 会漏掉它。

### 3. 依赖版本常量

升级运行时依赖时，需同时更新两处：

| 位置 | 常量 |
| :--- | :--- |
| `src/win/depend.rs` | `DWARFS_VERSION`、`OVLFS_VERSION` |
| `src/bin/post_build.rs` | `WINFSP_URL`/`WINFSP_HASH`、`DWARFS_URL`/`DWARFS_HASH`、`OVLFS_URL`/`OVLFS_HASH` |

`*_HASH` 必须是完整安装包的 SHA256（小写十六进制 64 位），已有测试校验格式。

### 4. 新增二进制目标

- `src/bin/*.rs` 会被 Cargo **自动发现**为二进制目标
- 若需要 feature 控制，必须在 `Cargo.toml` 显式声明 `[[bin]]` 与 `required-features`

---

## 提交信息规范

使用 [约定式提交](https://www.conventionalcommits.org/zh-hans/)：

```
<type>(<scope>): <简短描述>

[可选正文]

[可选脚注]
```

常用 `type`：

| type | 用途 |
| :--- | :--- |
| `feat` | 新功能 |
| `fix` | 修复缺陷 |
| `docs` | 仅文档改动 |
| `test` | 仅测试改动 |
| `refactor` | 重构，不改变行为 |
| `perf` | 性能优化 |
| `build` | 仅构建改动 |
| `ci` | 仅 CI 改动 |
| `chore` | 其他 |

`scope` 使用模块名，如 `dwarfs`、`payload`、`win/launch`、`cli`：

```
feat(cli): 支持 --set-manifest 替换 UAC 清单
fix(dwarfs): 修正 SegmentFile::open 在 offset 越界时的整数下溢
test(payload): 补充占位符内容的回归测试
docs: 补充可写层模式的对比说明
```

---

## 拉取请求流程

1. Fork 本仓库并克隆到本地
2. 创建分支：`git switch -c feature/your-feature`
3. 开发与自检（见下方清单）
4. 提交：遵循[提交信息规范](#提交信息规范)
5. 推送并创建 Pull Request，在描述中说明**改了什么、为什么改、如何验证**
6. 回应 Review 意见，保持提交历史整洁

### PR 检查清单

完整的检查清单内置在 [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md) 中，创建 PR 时会自动填充到描述框，请在提交前逐项勾选。

其中三项自动化检查必须本地通过：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features # 零警告
cargo test --all-features                 # 全部通过
```

自查要点（详见模板）：

- 公开方法有 `///` 文档注释，含参数与返回值说明
- 新增逻辑有单元测试，覆盖正常路径与边界条件
- 用户可见文案（错误信息、对话框、命令行输出）已在 `lang/` 下补充中英双语翻译
- 触及[需要特别留意的改动](#需要特别留意的改动)时，已在 PR 的「兼容性」章节说明

### 代码合并

- 审查正确性、边界条件、与现有架构的一致性、注释完整性
- 小改动（文档、单测）通常会被快速合并，大改动需谨慎审查
- 合并通常使用 rebase，过多 commit 使用 squash，不要使用 merge

---

## 发布流程（维护者）

1. 更新 `Cargo.toml` 中的 `version`
2. 自检：`cargo test --all-features`
3. 构建：`cargo build -r --all-features`
4. 后处理：`target/release/post_build`
5. 测试：打包一个示例应用并运行
6. 发布 Release（无需上传构建文件，会自动构建上传）

---

## 许可证

本项目基于 **GNU Lesser General Public License v3.0（LGPL-3.0）** 授权，详见 [LICENSE](./LICENSE)。

提交 Pull Request 即表示你同意：

- 你的贡献以 **LGPL-3.0** 授权给本项目
- 你有权提交该代码，且它不侵犯任何第三方的知识产权
- 若代码来自其他项目，你已在注释中标明来源与其许可证
