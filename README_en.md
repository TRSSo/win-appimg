# WinAppImg

> [简体中文](./README.md) | English

[![License: LGPL-3.0](https://img.shields.io/badge/License-LGPL--3.0-blue.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/Platform-Windows-blue.svg)
![Edition](https://img.shields.io/badge/rust-2024-orange.svg)
[![Release](https://img.shields.io/github/release/TRSSo/win-appimg)](https://github.com/TRSSo/win-appimg/releases/latest)

> A tool for creating Windows application images. Uses WinFsp + DwarFS, similar in principle to AppImage.

WinAppImg packages an entire application directory (or its DwarFS image) into a **single `.exe` file**. Users can run it directly — no installation, no extraction.

---

## Table of Contents

- [Introduction](#introduction)
- [Key Features](#key-features)
- [Tech Stack](#tech-stack)
- [Requirements](#requirements)
- [Usage](#usage)
- [Configuration](#configuration)
- [How It Works](#how-it-works)
- [License](#license)
- [Contributing](./CONTRIBUTING_en.md)

---

## Introduction

On Linux, [AppImage](https://appimage.org/) gives us "one file, one application". WinAppImg brings the same experience to Windows:

- **For packagers**: package an entire application directory into a single exe, with a customizable icon, version info and UAC manifest.
- **For users**: just run it. The runtime dependencies are installed on first launch (about 10 MiB).

Unlike a self-extracting archive, WinAppImg does **not** unpack files to disk:

```
Traditional SFX:  exe ──extract──> %TEMP%\xxx\ (all files) ──run──> app.exe
WinAppImg:        exe ──mount────> %TEMP%\virtual drive (on-demand, highly compressed) ──run──> app.exe
```

Application files always stay compressed inside the exe. The DwarFS file system decompresses them on demand, so a cold start only reads the blocks that are actually accessed.

### Artifacts

| File | Purpose |
| :--- | :--- |
| `win-appimg-cli` | **CLI packaging tool**: used by developers to build an application image from a directory |
| `win-appimg-gui` | **GUI packaging tool**: (in development) lets users archive applications themselves |
| `win-appimg-depend` | **Dependency manager**: optional GUI tool for installing / uninstalling the runtime dependencies |

---

## Key Features

### CLI Packaging Tool

- **Directory straight to image**: pass an application directory and the built-in packer recursively builds the DwarFS image; you can also call an external `mkdwarfs` with `--use-mkdwarfs`
- **Reuse an existing image**: if you have custom packaging requirements, pass a `.dwarfs` file and the packing step is skipped (note that if the file uses a compression algorithm other than ZSTD, compatibility mode will not work)
- **High compression ratio**: ZSTD compression, level 1–22 adjustable (`--compress-level`)
- **PE resource customization**:
  - Copy version info (`--copy-info`) and the icon (`--copy-icon`) from the startup executable
  - Manually set version info fields (`--set-info KEY=VALUE`) and the icon (`--set-icon`)
  - Request administrator privileges (`--req-admin`)
  - Show a console (`--show-console`)
- **Complete launch configuration**: supports launching an external program (`--external`), passing start arguments and setting the working directory (`--workdir`)

### Launcher

- **On-demand mounting**: mounts the application image through WinFsp + DwarFS and unmounts it on exit
- **Writable layer** (experimental): not recommended — change the program's behavior instead of writing to its own directory
  - `Save` — stored in a `.data` folder next to the exe, persisted
  - `Temp` — stored in a temporary directory, discarded on exit
  - `Memory` — stored in the MemFS in-memory file system
- **One-click dependency install**: on first launch it requests administrator privileges and installs everything in one go
- **Compatibility mode**: if the runtime dependencies cannot be installed or the program cannot run, it falls back to extracting to a temporary directory and launching from there
- **Startup repair**: when startup fails, a repair wizard offers reinstalling dependencies or switching to compatibility mode
- **Process group management**: the application and dependency processes are all placed in a Windows Job Object, so child processes are reliably reaped when the main process exits
- **Startup success detection**: configurable "exiting within N seconds of start counts as a startup failure"
- **Localized GUI**: TaskDialog dialogs with no console window flicker

---

## Tech Stack

### Language and Build

| Item | Notes |
| :--- | :--- |
| Language | Rust, Edition 2024 (requires Rust 1.85+) |
| Build system | Cargo, with a `build.rs` build script |

### Core Dependencies

| Dependency | Purpose |
| :--- | :--- |
| [`dwarfs`](https://crates.io/crates/dwarfs) | DwarFS image reading and extraction |
| [`dwarfs-enc`](https://crates.io/crates/dwarfs-enc) | DwarFS image packing (CDC chunking + deduplication + ZSTD) |
| [`editpe`](https://crates.io/crates/editpe) | PE file parsing and rewriting (resources, subsystem, section table) |
| [`zerompk`](https://crates.io/crates/zerompk) | MessagePack serialization (embedded configuration) |
| [`windows-sys`](https://crates.io/crates/windows-sys) | Win32 API bindings (Job Objects, IOCP, Shell, Registry) |

### Runtime Dependencies

| Component | Version requirement | Source |
| :--- | :--- | :--- |
| [WinFsp](https://github.com/winfsp/winfsp) | 2.2+ | MSI package installed silently by the install script |
| [DwarFS](https://github.com/mhx/dwarfs) | ≥ 0.15.7 | Mounts the image |
| [OverlayFS](https://github.com/TRSSo/WinFsp-OverlayFS) | ≥ 0.0.1 | Only needed when using a writable layer |

> After installation these take up about 10 MiB of disk space. The version requirements above are verified on every launch and an upgrade prompt is shown if they are not met. If installation is impossible, the launcher falls back to extracting to a temporary directory.

---

## Requirements

### Build Environment

- **Windows** supports all features
- **Non-Windows** cannot build the `launcher` and `depend` features
- **Rust 1.85 or newer** (Edition 2024)
- **curl** to download the runtime dependencies
- **7z** to extract the runtime dependencies

### Packaging Environment

- Windows, Linux, macOS or any other system supported by Rust
- `mkdwarfs` when using `--use-mkdwarfs` or when you have custom packaging requirements (optional)

### Runtime Environment

- Windows 7 or later
- Administrator privileges are required to install the runtime dependencies on first use (a UAC prompt appears)
- No special privileges are required in compatibility mode

---

## Usage

### Option 1: Download Prebuilt Artifacts (Recommended)

Download them from the [Releases](https://github.com/TRSSo/win-appimg/releases/latest) page

### Option 2: Build from Source

```bash
# 1. Clone the repository
git clone https://github.com/TRSSo/win-appimg.git
cd win-appimg

# 2. Build all components on Windows
cargo build -r --all-features
# Non-Windows cannot build all components
cargo build -r

# 3. Run the post-build step on Windows
target/release/post_build.exe
# Non-Windows needs the path to launcher.exe
target/release/post_build path/to/launcher.exe
```

### CLI Packaging Tool

Basic form:

```bash
win-appimg-cli <SOURCE> <OUTPUT> [OPTIONS] -- <COMMAND> [ARGS...]
```

- `<SOURCE>` — the application directory, or an already built `.dwarfs` image file
- `<OUTPUT>` — the output exe path; `.exe` is appended automatically if the extension differs
- Everything after `--` is the **startup command of the application inside the image**, stored verbatim and executed at runtime

#### Examples

> On Windows you need to replace the line continuation `\`: `^` in cmd, `` ` `` in PowerShell.

```bash
# Package an entire directory and launch app.exe inside it
win-appimg-cli myapp dist/MyApp -- app.exe

# Startup command with fixed arguments (arguments passed at runtime are appended)
win-appimg-cli myapp dist/MyApp -- app.exe --config conf.ini

# Set the working directory to the program directory
win-appimg-cli myapp dist/MyApp -w . -- app.exe

# Set the working directory to a subdirectory of the program directory
win-appimg-cli myapp dist/MyApp -w data -- app.exe

# Reuse an existing DwarFS image and skip packing
win-appimg-cli app.dwarfs dist/MyApp -- app.exe

# Lower the compression level to speed up packing (default 22; use 3~7 for speed)
win-appimg-cli myapp dist/MyApp -c 7 -- app.exe

# Customize PE resources
win-appimg-cli myapp dist/MyApp \
  --set-info FileVersion=1.2.3.0 \
  --set-info ProductName=MyApp \
  --set-info FileDescription="My Application" \
  --set-icon myapp.ico \
  -- app.exe

# Copy resources from the startup executable
win-appimg-cli myapp dist/MyApp --copy-info --copy-icon -- app.exe

# Programs that need administrator privileges
win-appimg-cli myapp dist/MyApp --req-admin -- app.exe

# Package a command-line program
win-appimg-cli myapp dist/MyApp --show-console -- app.exe

# Writable layer stored in MyApp.data next to the exe, kept after exit
win-appimg-cli myapp dist/MyApp --write -- app.exe

# Writable layer on a RAM disk, discarded on exit
win-appimg-cli myapp dist/MyApp --write-memory -- app.exe

# Do not embed dependencies (saves about 3 MiB; downloaded on demand at runtime)
win-appimg-cli myapp dist/MyApp --no-depend -- app.exe
```

### Running the Packaged App

Normal programs can be run directly. Command-line programs also accept arguments, which are appended to the ones set at build time:

```bash
# Specified at build time: app.exe --config conf.ini
MyApp.exe --verbose
# Actually executed: app.exe --config conf.ini --verbose
```

### Managing Dependencies

Run `win-appimg-depend` and choose `Install` or `Uninstall` in the window that appears

```
Current status: Installed
──────────────────--------
  [Install]  [Uninstall]
```

> Install and uninstall logs are saved to `<temp directory>\install.log`; on failure you will be asked whether to view them.

---

## Configuration

### Cargo Features

| Feature | Default | Notes |
| :--- | :--- | :--- |
| `cli` | ✅ | Builds the `cli` packaging command-line tool |
| `depend` | ❌ | Builds the `depend` dependency manager and makes `post_build` append the dependency package to it |
| `launcher` | ❌ | Builds the `launcher` application image entry point, which `post_build` embeds into `cli` |

### Command-Line Options

#### Positional Arguments

| Argument | Notes |
| :--- | :--- |
| `<SOURCE>` | Source file path. A directory is packed recursively; a file is treated as an already built DwarFS image |
| `<OUTPUT>` | Output file path. `.exe` is appended automatically if the extension differs |
| `[COMMAND]...` | The startup command and fixed arguments after `--`; arguments passed at runtime are appended to them |

#### Options

| Option | Default | Notes |
| :--- | :--- | :--- |
| `-w, --workdir <DIR>` | — | Application working directory; relative paths are resolved against the image mount point |
| `-c, --compress-level <1-22>` | `22` | ZSTD compression level. Higher values mean a smaller size and slower packing |
| `-e, --external` | off | Launch an external program instead of the one inside the image |
| `--no-depend` | off | Do not embed the WinFsp dependency package; prompt for download on demand at runtime |
| `--show-console` | off | Show a console window (sets the PE subsystem to CUI) |
| `--req-admin` | off | Replace `asInvoker` with `requireAdministrator` in the manifest |
| `--copy-info` | off | Copy the version info resource from the startup executable |
| `--copy-icon` | off | Copy the icon resource from the startup executable |
| `--set-info <KEY=VALUE>` | — | Set a version info field; can be passed multiple times |
| `--set-icon <FILE>` | — | Set the icon file |
| `--set-manifest <FILE>` | — | Replace the UAC manifest file |
| `--use-mkdwarfs` | off | Call an external `mkdwarfs` to build the image instead of the built-in packer |
| `--started-time <SECOND>` | `0` | Exiting within this many seconds of start counts as a startup failure; `0` disables the check |
| `--write` | off | Save the writable layer to a `.data` folder next to the exe (experimental) |
| `--write-temp` | off | Save the writable layer to a temporary directory, discarded on exit (experimental) |
| `--write-memory` | off | Save the writable layer to the in-memory file system (experimental) |
| `-h, --help` | — | Show help (`-h` for the short version) |
| `-V, --version` | — | Show the version |

#### Keys Supported by `--set-info`

Any Windows version info string can be set (for example `ProductName`, `CompanyName`, `FileDescription`, `LegalCopyright`).

In addition to being written to the string table, `FileVersion` and `ProductVersion` are also parsed into the PE binary version number:

```bash
--set-info FileVersion=1.2.3.4      # -> major = 1<<16 | 2, minor = 3<<16 | 4
```

Parsing rules: split on `.`, parse each segment as a `u16`, invalid segments are treated as `0`, and at most the first 4 segments are used.

### Writable Layer Modes

| Mode | Option | Storage | After exit | Typical scenario |
| :--- | :--- | :--- | :--- | :--- |
| Read-only | (default) | — | — | Most applications that do not write to their own directory |
| `Save` | `--write` | `<name>.data` next to the exe | Kept | Files written to the program directory need to be kept |
| `Temp` | `--write-temp` | Temporary directory | Discarded | Writes temporary files at runtime but nothing needs to persist |
| `Memory` | `--write-memory` | MemFS RAM disk | Discarded | Frequent writes that should not touch the disk |

> The writable layer is currently **experimental**, because the OverlayFS it depends on is an experimental version. Change the program's behavior instead of writing to its own directory.

---

## How It Works

### Artifact Layout

```
┌───────────────────────────────────┐
│ launcher.exe (PE image)           │  The launcher itself
│   └─ .payload: offset + length    │  Points to LauncherPayload
├───────────────────────────────────┤
│ Runtime dependencies (optional)   │
├───────────────────────────────────┤
│ Application DwarFS image          │
├───────────────────────────────────┤
│ LauncherPayload (MessagePack)     │  Launch config + offset/length/hash of each section
└───────────────────────────────────┘
```

> The `.payload` section is a 16-byte placeholder `PAYLOAD\u{F05F}OFFSET` in the source; at build time `post_build` and `cli` write the real offset and length back into it. At runtime the launcher reads that structure with `read_volatile` to locate and deserialize the configuration at the end of the file.

### Startup Flow

```
User starts MyApp.exe
    │
    ├─ Read .payload section → deserialize LauncherPayload
    │
    ├─ Check runtime dependencies
    │      └─ Missing → dialog → extract embedded deps → silent install via UAC
    │                          └─ Not embedded → dialog prompting download
    │
    ├─ Mount the image to a temp directory (polls for the mount point, ~100s timeout)
    │
    ├─ Start the application process (inherit the console or hide the window)
    │      └─ If started_time is set, exiting within it counts as a startup failure
    │
    └─ Wait for any process group to exit
           ├─ App exits → stop dependencies, clean up temp files, return its exit code
           └─ Startup failure / dependency exit → report the error and open the repair wizard
```

> Every child process is assigned to its own Windows Job Object, all associated with the same IOCP. The job is created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so even if the launcher is killed, the application and the mount process are reaped by the system and no mount point is left behind.

---

## License

This project is licensed under the **GNU Lesser General Public License v3.0 (LGPL-3.0)**. See [LICENSE](./LICENSE) for details.

### Third-Party Components

Packaged artifacts use the following components at runtime, each under its own license:

| Component | License |
| :--- | :--- |
| [WinFsp](https://github.com/winfsp/winfsp) | GPL-3.0 (with the FLOSS exception) |
| [DwarFS](https://github.com/mhx/dwarfs) | MIT |
| [WinFsp-OverlayFS](https://github.com/TRSSo/WinFsp-OverlayFS) | LGPL-3.0 |

> When distributing an application packaged with WinAppImg, please respect the license requirements of the components above. WinAppImg itself does not ship any GPL code other than WinFsp with the artifact.
