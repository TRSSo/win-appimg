use std::{
  env::var_os,
  ffi::OsString,
  fs::{self, File},
  io::{Error, ErrorKind, Result},
  os::windows::process::CommandExt,
  path::{Path, PathBuf},
  process::{Command, Stdio},
};

use crate::{
  include_lang, okcancel,
  payload::PayloadData,
  retry,
  win::{
    dialog::{self, open_url, show_path},
    sudo,
  },
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

include_lang!();

/// `dwarfs.exe` 的最低版本要求
const DWARFS_VERSION: (u32, u32, u32) = (0, 15, 7);
/// `ovlfs.exe` 的最低版本要求
const OVLFS_VERSION: (u32, u32, u32) = (0, 0, 1);

/// 依赖程序的安装路径
pub struct DependPath {
  /// WinFsp 运行库 `winfsp-<arch>.dll`
  pub winfsp_dll_path: PathBuf,
  /// `dwarfs.exe`，用于挂载镜像
  pub dwarfs_path: PathBuf,
  /// `memfs-<arch>.exe`，内存文件系统，用于内存可写层
  pub memfs_path: PathBuf,
  /// `ovlfs.exe`，OverlayFS，用于叠加可写层
  pub ovlfs_path: PathBuf,
}

/// 检查依赖是否已安装，返回各依赖程序的路径
///
/// `check_ovlfs` 为 `true` 时额外校验 `ovlfs.exe` 的版本。
///
/// 未找到 WinFsp 或缺少任一依赖时返回 `ErrorKind::NotFound`，
/// 版本低于要求时返回 `ErrorKind::Unsupported`，无法解析版本时返回 `ErrorKind::InvalidData`。
pub fn check_depend(check_ovlfs: bool) -> Result<DependPath> {
  let mut program_paths = Vec::new();
  if let Some(path) = var_os("ProgramFiles(x86)") {
    program_paths.push(path)
  };
  if let Some(path) = var_os("ProgramFiles") {
    program_paths.push(path)
  };
  program_paths.push(OsString::from(r"C:\Program Files (x86)"));
  program_paths.push(OsString::from(r"C:\Program Files"));

  #[cfg(target_arch = "x86_64")]
  let arch = "x64";
  #[cfg(target_arch = "x86")]
  let arch = "x86";
  #[cfg(target_arch = "aarch64")]
  let arch = "a64";

  let winfsp_path = {
    let mut path = None;
    for i in program_paths {
      let p = Path::new(&i).join(r"WinFsp\bin\");
      if p.exists() {
        path = Some(p);
        break;
      }
    }
    if let Some(path) = path {
      path
    } else {
      return Err(Error::new(ErrorKind::NotFound, m!(NotFoundWinFsp)));
    }
  };

  let depend_path = DependPath {
    winfsp_dll_path: winfsp_path.join(format!("winfsp-{arch}.dll")),
    dwarfs_path: winfsp_path.join("dwarfs.exe"),
    memfs_path: winfsp_path.join(format!("memfs-{arch}.exe")),
    ovlfs_path: winfsp_path.join("ovlfs.exe"),
  };

  for i in [&depend_path.winfsp_dll_path, &depend_path.dwarfs_path] {
    if !i.exists() {
      return Err(Error::new(
        ErrorKind::NotFound,
        m!(NotFoundFile, i.display()),
      ));
    }
  }

  // dwarfs (v0.15.6 [2026-07-26], fuse version 32)
  check_version(
    "dwarfs",
    "dwarfs (v",
    &command_no_window(&depend_path.dwarfs_path)?,
    DWARFS_VERSION,
  )?;

  if check_ovlfs {
    for i in [&depend_path.memfs_path, &depend_path.ovlfs_path] {
      if !i.exists() {
        return Err(Error::new(
          ErrorKind::NotFound,
          m!(NotFoundFile, i.display()),
        ));
      }
    }

    // WinFsp OverlayFS v0.0.1
    check_version(
      "ovlfs",
      "WinFsp OverlayFS v",
      &command_no_window(&depend_path.ovlfs_path)?,
      OVLFS_VERSION,
    )?;
  }

  Ok(depend_path)
}

/// 以 `-h` 参数运行 `path` 并收集其全部输出，不显示控制台窗口
///
/// 进程退出码非 0 时返回 `ErrorKind::ConnectionAborted`，程序不存在时返回 `ErrorKind::NotFound`。
pub fn command_no_window<P: AsRef<Path>>(path: P) -> Result<String> {
  let output = Command::new(path.as_ref())
    .arg("-h")
    .creation_flags(CREATE_NO_WINDOW)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()?;

  let output_str = String::from_utf8_lossy(&output.stdout).to_string()
    + "\n"
    + &String::from_utf8_lossy(&output.stderr);

  if !output.status.success() {
    return Err(Error::new(
      ErrorKind::ConnectionAborted,
      m!(RunError, path.as_ref().display(), output_str),
    ));
  }

  Ok(output_str)
}

/// 在 `output` 中查找 `pattern`，解析其后的版本号并与 `minver` 比较
///
/// 逐行扫描，命中 `pattern` 的第一行即参与比较：
/// 版本低于 `minver` 返回 `ErrorKind::Unsupported`，
/// 找不到或解析不出版本返回 `ErrorKind::InvalidData`。
fn check_version(name: &str, pattern: &str, output: &str, minver: (u32, u32, u32)) -> Result<()> {
  for line in output.lines() {
    let line = line.trim();
    let Some(start) = line.find(pattern) else {
      continue;
    };
    let after_v = &line[start + pattern.len()..];
    let version: String = after_v
      .chars()
      .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
      .collect();
    if version.is_empty() {
      continue;
    }
    if let Some(version) = parse_version(&version) {
      if version < minver {
        return Err(Error::new(
          ErrorKind::Unsupported,
          m!(VersionTooLow, name, version.0, version.1, version.2),
        ));
      }
      return Ok(());
    }
  }
  Err(Error::new(
    ErrorKind::InvalidData,
    m!(VersionParseError, name, output),
  ))
}

/// 解析 `a.b.c` 形式的版本号，缺失的小版本与补丁版本按 0 处理
///
/// 任一段不是合法的 `u32` 时返回 `None`；多余的段被忽略。
fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
  let mut split = version.split('.');
  let major = split.next()?.parse().ok()?;
  let minor = match split.next() {
    Some(v) => v.parse().ok()?,
    None => 0,
  };
  let patch = match split.next() {
    Some(v) => v.parse().ok()?,
    None => 0,
  };
  Some((major, minor, patch))
}

/// 把 `file` 中的依赖包解压到 `depend_path`，失败时弹窗询问是否重试
///
/// 返回：
/// - `Some(true)`：解压并校验通过
/// - `Some(false)`：`payload` 为空，已引导用户手动下载安装，调用方应重新检查依赖
/// - `None`：用户取消
pub fn extract_depend(
  path: &Path,
  file: &mut File,
  payload: &PayloadData,
  depend_path: &Path,
) -> Option<bool> {
  if payload.offset == 0 || payload.length == 0 {
    let depend_url = format!("{}/releases/latest", env!("CARGO_PKG_REPOSITORY"));
    let _ = open_url(&depend_url);
    return if okcancel!(
      m!(ExtractDependTitle),
      m!(ExtractDepend),
      &m!(ExtractDependOpenUrl, depend_url)
    ) {
      Some(false)
    } else {
      None
    };
  }

  while let Err(err) = payload.extract(path, file, depend_path) {
    if !retry!(m!(ExtractDependTitle), &m!(ExtractDependRetry, err)) {
      return None;
    }
    let _ = fs::remove_dir_all(depend_path);
  }
  Some(true)
}

/// 以管理员权限运行 `depend_path/install.cmd` 安装依赖，失败时弹窗询问是否重试
///
/// 安装成功并通过校验时返回依赖路径，用户取消或最终仍失败时返回 `None`。
pub fn install_depend(depend_path: &Path) -> Option<DependPath> {
  loop {
    let err = match sudo::run(&depend_path.join("install.cmd")).and_then(|code| {
      if code == 0 {
        check_depend(true)
      } else {
        Err(Error::new(
          ErrorKind::Interrupted,
          m!(ProcessExitCode, code),
        ))
      }
    }) {
      Ok(p) => return Some(p),
      Err(err) => err,
    };

    if !retry!(
      m!(InstallDependTitle),
      m!(InstallDependRetry),
      &err.to_string()
    ) {
      break;
    }
  }

  let log_path = depend_path.join("install.log");
  if log_path.exists() && okcancel!(m!(InstallDependTitle), m!(ViewInstallLog)) {
    let _ = show_path(&log_path);
  }
  None
}

/// 以管理员权限运行 `depend_path/uninstall.cmd` 卸载依赖，失败时弹窗询问是否重试
///
/// 返回依赖是否已不再处于安装状态。
/// 卸载脚本执行失败但依赖确实已不存在时，同样视为成功返回 `true`。
pub fn uninstall_depend(depend_path: &Path) -> bool {
  loop {
    let res = sudo::run(&depend_path.join("uninstall.cmd")).and_then(|code| {
      if code == 0 {
        Ok(())
      } else {
        Err(Error::new(
          ErrorKind::Interrupted,
          m!(ProcessExitCode, code),
        ))
      }
    });
    let depend = match check_depend(true) {
      Ok(_) => Err(Error::new(
        ErrorKind::AlreadyExists,
        m!(DependStillInstalled),
      )),
      Err(_) => Ok(()),
    };

    match (res, depend) {
      (Ok(_), Ok(_)) => return true,
      (Ok(_), Err(err)) | (Err(err), Err(_)) => {
        if !retry!(
          m!(UninstallDependTitle),
          m!(UninstallDependRetry),
          &err.to_string()
        ) {
          break;
        }
      }
      (Err(err), Ok(_)) => {
        if !retry!(
          m!(UninstallDependTitle),
          m!(UninstallDependRetryNotInstalled),
          &err.to_string()
        ) {
          return true;
        }
      }
    }
  }

  let log_path = depend_path.join("install.log");
  if log_path.exists() && okcancel!(m!(UninstallDependTitle), m!(ViewUninstallLog)) {
    let _ = show_path(&log_path);
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_version_full() {
    assert_eq!(parse_version("0.15.7"), Some((0, 15, 7)));
    assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
    assert_eq!(parse_version("10.0.19041"), Some((10, 0, 19041)));
  }

  #[test]
  fn parse_version_missing_parts_default_to_zero() {
    assert_eq!(parse_version("1"), Some((1, 0, 0)));
    assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
    assert_eq!(parse_version("0"), Some((0, 0, 0)));
  }

  #[test]
  fn parse_version_ignores_extra_parts() {
    assert_eq!(parse_version("1.2.3.4"), Some((1, 2, 3)));
  }

  #[test]
  fn parse_version_rejects_invalid_input() {
    for version in [
      "", ".", "..", "a.b.c", "1.b", "1.2.c", "-1", "1.2.-3", " 1", "1 ",
    ] {
      assert_eq!(parse_version(version), None, "version = {version:?}");
    }
  }

  #[test]
  fn parse_version_accepts_leading_plus_sign() {
    // 与 Rust 整数解析一致，允许前导 `+`
    assert_eq!(parse_version("+1"), Some((1, 0, 0)));
  }

  #[test]
  fn parse_version_rejects_overflow() {
    assert_eq!(parse_version("4294967296"), None);
    assert_eq!(parse_version("1.4294967296"), None);
  }

  #[test]
  fn parse_version_accepts_max_u32() {
    assert_eq!(
      parse_version("4294967295.4294967295.4294967295"),
      Some((u32::MAX, u32::MAX, u32::MAX))
    );
  }

  #[test]
  fn parse_version_rejects_empty_segments() {
    // 空段无法解析为 `u32`，整段按失败处理
    assert_eq!(parse_version("1..2"), None);
    assert_eq!(parse_version("1.2."), None);
    assert_eq!(parse_version(".1"), None);
    assert_eq!(parse_version("1."), None);
  }

  #[test]
  fn check_version_rejects_prerelease_suffix() {
    // `take_while` 会保留 `-`，补丁段 `7-rc1` 无法解析，因此整体失败
    let output = "dwarfs (v0.15.7-rc1)";
    let err = check_version("dwarfs", "dwarfs (v", output, DWARFS_VERSION).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
  }

  #[test]
  fn check_version_uses_first_matching_line() {
    // 命中模式的行直接参与比较，不会继续扫描后面版本更高的行
    let output = "dwarfs (v0.1.0)\ndwarfs (v9.9.9)";
    let err = check_version("dwarfs", "dwarfs (v", output, (1, 0, 0)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);
    assert_eq!(err.to_string(), m!(VersionTooLow, "dwarfs", 0, 1, 0));
  }

  #[test]
  fn check_version_skips_line_with_empty_version() {
    // 版本号为空的行会被跳过，继续扫描后续行
    let output = "x v\nx v2.0.0";
    assert!(check_version("x", "x v", output, (2, 0, 0)).is_ok());
  }

  #[test]
  fn check_version_accepts_equal_or_newer() {
    let output = "dwarfs (v0.15.7 [2026-07-26], fuse version 32)";
    assert!(check_version("dwarfs", "dwarfs (v", output, DWARFS_VERSION).is_ok());
    assert!(check_version("dwarfs", "dwarfs (v", output, (0, 15, 6)).is_ok());
    assert!(check_version("dwarfs", "dwarfs (v", output, (0, 0, 1)).is_ok());
  }

  #[test]
  fn check_version_rejects_older_version() {
    let output = "dwarfs (v0.15.7 [2026-07-26], fuse version 32)";
    let err = check_version("dwarfs", "dwarfs (v", output, (0, 15, 8)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);
    assert_eq!(err.to_string(), m!(VersionTooLow, "dwarfs", 0, 15, 7));
  }

  #[test]
  fn check_version_rejects_lower_ovlfs_version() {
    let output = "WinFsp OverlayFS v0.0.1";
    let err = check_version("ovlfs", "WinFsp OverlayFS v", output, (0, 0, 2)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);
    assert_eq!(err.to_string(), m!(VersionTooLow, "ovlfs", 0, 0, 1));
  }

  #[test]
  fn check_version_compares_numerically_not_lexicographically() {
    // 0.15.10 按数值比较高于 0.15.7，按字符串比较则会误判
    let output = "dwarfs (v0.15.10)";
    assert!(check_version("dwarfs", "dwarfs (v", output, DWARFS_VERSION).is_ok());
    assert!(check_version("dwarfs", "dwarfs (v", output, (0, 15, 9)).is_ok());
  }

  #[test]
  fn check_version_scans_multiple_lines() {
    let output = "usage: ovlfs [options]\nWinFsp OverlayFS v0.0.1\noptions:";
    assert!(check_version("ovlfs", "WinFsp OverlayFS v", output, OVLFS_VERSION).is_ok());
  }

  #[test]
  fn check_version_accepts_ovlfs_actual_output() {
    let output = "WinFsp OverlayFS v0.0.1";
    assert!(check_version("ovlfs", "WinFsp OverlayFS v", output, OVLFS_VERSION).is_ok());
  }

  #[test]
  fn check_version_rejects_missing_pattern() {
    let output = "some other program v1.2.3";
    for name in ["dwarfs", "ovlfs"] {
      let err = check_version(name, "dwarfs (v", output, (0, 0, 1)).unwrap_err();
      assert_eq!(err.kind(), ErrorKind::InvalidData);
      assert_eq!(err.to_string(), m!(VersionParseError, name, output));
    }
  }

  #[test]
  fn check_version_rejects_unparsable_version() {
    let output = "dwarfs (vunknown build)";
    let err = check_version("dwarfs", "dwarfs (v", output, DWARFS_VERSION).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
  }

  #[test]
  fn check_version_stops_scanning_at_whitespace() {
    // 版本号之后的空白字符会终止扫描
    let output = "dwarfs (v0.15.7 [2026-07-26])";
    assert!(check_version("dwarfs", "dwarfs (v", output, DWARFS_VERSION).is_ok());
  }

  #[test]
  fn check_version_accepts_short_version() {
    assert!(check_version("x", "x v", "x v2", (2, 0, 0)).is_ok());
    assert!(check_version("x", "x v", "x v2.1", (2, 1, 0)).is_ok());
    // 缺少小版本时按 0 处理，因此低于 2.1.1
    assert!(check_version("x", "x v", "x v2.1", (2, 1, 1)).is_err());
  }

  #[test]
  fn command_no_window_rejects_missing_program() {
    let err = command_no_window("win-appimg-not-a-real-program.exe").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
  }
}
