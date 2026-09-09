# Contributing

> [简体中文](./CONTRIBUTING.md) | English

Thank you for your interest in contributing to WinAppImg!

This project is a Windows application image packaging tool written in Rust. It touches fairly low-level topics such as the PE file structure, Windows file system drivers (WinFsp), and process / Job Object management. Whether you want to fix a typo, add tests or implement a new feature, you are welcome to join in.

This document covers **setting up a development environment, code style, and the commit and PR workflow**. If you only want to learn how to use the tool, see [README_en.md](./README_en.md).

## Table of Contents

- [How You Can Contribute](#how-you-can-contribute)
- [Development Environment](#development-environment)
- [Building and Testing](#building-and-testing)
- [Code Style](#code-style)
- [Changes Needing Extra Care](#changes-needing-extra-care)
- [Commit Message Convention](#commit-message-convention)
- [Pull Request Process](#pull-request-process)
- [Release Process (Maintainers)](#release-process-maintainers)
- [License](#license)

---

## How You Can Contribute

This repository provides GitHub Issue templates in [`.github/ISSUE_TEMPLATE/`](.github/ISSUE_TEMPLATE/):

| Template | Purpose |
| :--- | :--- |
| `bug_report.md` | **Bug report** — packing / startup / dependency / writable layer / build anomalies |
| `feature_request.md` | **Feature request** — new features or improvements |
| `question.md` | **Question** — questions about usage |

Choose one from the repository's [New Issue](https://github.com/TRSSo/win-appimg/issues/new/choose) page. The templates already contain the environment information table and confirmation items — please fill them in as completely as you can.

### Reporting a Bug

Please search for an existing issue first. The key information a bug report needs:

| Item | Notes |
| :--- | :--- |
| Environment | Windows version and patch level, CPU architecture (x64 / x86 / ARM64) |
| Version | The version of this project; please report against the latest release if possible |
| Component versions | The versions of WinFsp and DwarFS, plus any other components you use |
| Steps to reproduce | The complete command line and the structure of the input directory |
| Actual output | The complete output when packing or running; use `--show-console` to enable the runtime log |

> For packing failures, attach the complete output of `cli`; for startup failures, attach the error text from the dialog; for dependency installation, attach `install.log`.

### Reporting a Security Issue

Please **do not** open a public issue for security vulnerabilities. Use "Report a vulnerability" on the repository's Security page instead.

### Proposing a New Feature

Please open an issue first describing the need and the use case, to avoid duplicated work or going in the wrong direction. The following topics are especially welcome:

- Further optimization of package size and startup speed
- A more complete writable layer solution
- Better repair logic for startup failures
- Build support for non-Windows platforms

### Other

- Improving documentation and adding examples
- Adding unit tests (especially for edge cases)
- Translation and copy-editing

---

## Development Environment

### Required

| Tool | Version | Notes |
| :--- | :--- | :--- |
| Rust | **1.85+** | The project uses Edition 2024 |
| Windows | 7+ | See "About non-Windows systems" below |

### Optional

Only needed when **re-downloading and packing the dependencies** (that is, when running `post_build` with no local cache):

| Tool | Purpose |
| :--- | :--- |
| `curl` | Download the runtime dependencies |
| `7z` | Extract the runtime dependencies |
| `mkdwarfs` | Only needed when debugging the `--use-mkdwarfs` path |

### About Non-Windows Systems

The [win module](./src/win/) contains parts that only compile on Windows.

Some features can only be built on Windows, for example `launcher` and `depend`.

| System | Recommended commands | Notes |
| :--- | :--- | :--- |
| Windows | `cargo clippy --all-targets --all-features`<br>`cargo test --all-features` | Can build and test every target |
| Linux / macOS | `cargo clippy --all-targets`<br>`cargo test` | **Only the default features** |

**Please make sure your change still passes `cargo check` on non-Windows systems** (default features).

When a common module (such as `dwarfs.rs` or `payload.rs`) needs to branch by platform, follow the three-branch pattern of `dwarfs.rs::create_symlink`:

```rust
#[cfg(unix)]
fn create_symlink(..) { .. }

#[cfg(windows)]
fn create_symlink(..) { .. }

// Fallback branch so other platforms still compile
#[cfg(not(any(unix, windows)))]
fn create_symlink(..) { .. }
```

---

## Building and Testing

### Features

| Feature | Default | Binary built |
| :--- | :--- | :--- |
| `cli` | ✅ | The packaging command-line tool |
| `depend` | ❌ | The dependency manager GUI |
| `launcher` | ❌ | The application image entry point, embedded into `cli` |

```bash
# Everyday development (cli only, fastest to compile)
cargo build

# Full check: all features, all targets (including test code)
cargo clippy --all-targets --all-features

# Build the complete release artifacts
cargo build -r --all-features
```

### Building the Complete Artifacts

A plain `cargo build` produces binaries that do not yet contain the dependency package and the launcher, so the post-build step must be run as well:

```bash
target/release/post_build.exe
# Non-Windows needs the path to launcher.exe
target/release/post_build path/to/launcher.exe
```

It downloads and verifies the dependency package, packs it into `depend.dwarfs`, then appends the dependencies and `launcher.exe` to the artifacts.

> The dependency package is cached in the build directory and reused directly after a SHA256 check on subsequent runs.
>
> Note: when the [res directory](./res/) is missing, `build.rs` skips embedding the icon and manifest. This is only intended for use as a dependency of another project — release artifacts must include the manifest.

### Testing

All current unit tests live in the `#[cfg(test)] mod tests` block at the end of each source file. When adding tests, please follow these rules:

- **Location**: a `mod tests` in the same file as the code under test, using `use super::*;`
- **Isolation**: create temporary directories / files with `tempfile`; do not depend on real paths and do not leave residue in the repository
- **Naming**: `function_under_test_scenario_expected_result`, for example `segment_file_open_rejects_offset_beyond_size`
- **Comments**: use `//` above the assertion to explain **why that value is expected**, especially for edge cases and counter-intuitive branches
- **Platform**: mark tests that can only run on Windows with `#[cfg(windows)]` (not needed inside the [win module](./src/win/)); use `#[cfg(any(unix, windows))]` for tests spanning unix / windows

---

## Code Style

### Formatting and Linting

- Run `cargo fmt` before committing; the configuration is fixed by `rustfmt.toml`
- `cargo clippy --all-targets --all-features` must produce **zero warnings**. If an exemption is truly necessary, add `#[allow(..)]` and explain why in a comment

### Comments

- Public methods, public types and key private methods all need `///` doc comments
- Doc comments must describe the **purpose, parameters and return value**, and list the `ErrorKind` for each error branch
- Supplement complex logic with `//` line comments explaining **why it is done this way**, rather than restating the code

```rust
/// Opens `path` and exposes the byte range starting at `offset` with length `length` as an independent file
///
/// Returns `ErrorKind::InvalidInput` when `offset` exceeds the file size or `length` exceeds `[offset, file size)`.
pub fn open<P: AsRef<Path>>(path: P, offset: u64, length: u64) -> Result<Self> {
```

### Naming and Language

- Identifiers are written in English
- Doc comments and inline comments are written in **Chinese**
- GUI programs print output only under `#[cfg(debug_assertions)]`

### Internationalization

All user-visible text (error messages, dialogs, command-line output) goes through the `m!` macro. The Chinese and English versions live together in the `*.yml` files of the [lang directory](./lang/), with each key mapping to `[English, Chinese]`:

For every `*.yml`, `build.rs` generates a `.rs` file of the same name under `OUT_DIR/i18n/` (e.g. `lang/lib.yml` -> `i18n/src/lib.rs`), which is pulled in by the `include_lang!()` call at the top of the source file. Text containing `{}` placeholders produces a formatting macro, and **the placeholders must match on both the English and Chinese sides**.
After adding new text, remember to include `include_lang!()` at the beginning of the file and call `init_lang()` at the program entry point.

### Error Handling

- Use `std::io::Result<T>` and `std::io::Error` consistently; do not introduce a custom error type
- Use semantically accurate `ErrorKind`s: `NotFound`, `InvalidInput`, `InvalidData`, `UnexpectedEof`, `Unsupported`, `PermissionDenied`, `Interrupted`, etc.
- When wrapping third-party errors, add i18n context:

  ```rust
  .map_err(|e| Error::other(m!(AddFileError, e)))?;
  ```

- Watch out for integer overflow in arithmetic: compare before subtracting, or use `saturating_sub` / `checked_sub`

### Platform-Specific Code

- `src/win/` is a Windows-only module; code there does not need to consider other platforms
- When adding a platform branch to a common module, you must provide a `#[cfg(not(any(unix, windows)))]` fallback
- Before adding a Windows API call, make sure the corresponding `windows-sys` feature is enabled in `Cargo.toml` (for example `Win32_System_JobObjects`)

---

## Changes Needing Extra Care

### 1. MessagePack Struct Fields

`PayloadData`, `Payload`, `LauncherPayload` and others are serialized at the end of the artifact.

**Adding, removing or reordering fields breaks deserialization of older artifacts.** When testing on non-Windows platforms, make sure you build the latest `launcher.exe` on Windows.

### 2. `include` in `Cargo.toml`

When adding a **non-`.rs` file** that must be published with the crate, you must update this entry, otherwise `cargo package` will omit it.

### 3. Dependency Version Constants

When upgrading a runtime dependency, two places must be updated together:

| Location | Constants |
| :--- | :--- |
| `src/win/depend.rs` | `DWARFS_VERSION`, `OVLFS_VERSION` |
| `src/bin/post_build.rs` | `WINFSP_URL`/`WINFSP_HASH`, `DWARFS_URL`/`DWARFS_HASH`, `OVLFS_URL`/`OVLFS_HASH` |

`*_HASH` must be the SHA256 of the complete installer (64 lowercase hexadecimal characters); the test validates the format.

### 4. Adding a Binary Target

- `src/bin/*.rs` files are **auto-discovered** by Cargo as binary targets
- If the target needs to be feature-gated, you must declare `[[bin]]` and `required-features` explicitly in `Cargo.toml`

---

## Commit Message Convention

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short description>

[optional body]

[optional footer]
```

Common `type` values:

| type | Purpose |
| :--- | :--- |
| `feat` | A new feature |
| `fix` | A bug fix |
| `docs` | Documentation-only changes |
| `test` | Test-only changes |
| `refactor` | Refactoring without behavior changes |
| `perf` | Performance improvements |
| `build` | Build-only changes |
| `ci` | CI-only changes |
| `chore` | etc. |

`scope` uses the module name, such as `dwarfs`, `payload`, `win/launch`, `cli`:

```
feat(cli): 支持 --set-manifest 替换 UAC 清单
fix(dwarfs): 修正 SegmentFile::open 在 offset 越界时的整数下溢
test(payload): 补充占位符内容的回归测试
docs: 补充可写层模式的对比说明
```

---

## Pull Request Process

1. Fork this repository and clone it locally
2. Create a branch: `git switch -c feature/your-feature`
3. Develop and self-check (see the checklist below)
4. Commit: follow the [Commit Message Convention](#commit-message-convention)
5. Push and open a Pull Request, describing **what changed, why, and how to verify it**
6. Respond to review comments and keep the commit history clean

### PR Checklist

The full checklist is built into [`.github/PULL_REQUEST_TEMPLATE_en.md`](.github/PULL_REQUEST_TEMPLATE_en.md) and is filled into the description box automatically when you create a PR. Please go through it item by item before submitting.

Three automated checks must pass locally:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features # zero warnings
cargo test --all-features                 # all passing
```

Self-check points (see the template for details):

- Public methods have `///` doc comments covering parameters and return values
- New logic has unit tests covering both the normal path and edge cases
- User-visible text (error messages, dialogs, command-line output) has Chinese and English translations in `lang/`
- If the change touches [Changes Needing Extra Care](#changes-needing-extra-care), it is described in the "Compatibility" section of the PR

### Merging

- We review correctness, edge cases, consistency with the existing architecture, and comment completeness
- Small changes (documentation, unit tests) are usually merged quickly; large changes need careful review
- Merging usually uses rebase; squash when there are too many commits. Do not use merge commits

---

## Release Process (Maintainers)

1. Bump `version` in `Cargo.toml`
2. Self-check: `cargo test --all-features`
3. Build: `cargo build -r --all-features`
4. Post-build: `target/release/post_build`
5. Test: package a sample application and run it
6. Publish the release (no need to upload build files; they are built and uploaded automatically)

---

## License

This project is licensed under the **GNU Lesser General Public License v3.0 (LGPL-3.0)**. See [LICENSE](./LICENSE) for details.

By submitting a Pull Request you agree that:

- Your contribution is licensed to this project under **LGPL-3.0**
- You have the right to submit the code and it does not infringe any third party's intellectual property
- If the code comes from another project, you have noted its source and license in a comment
