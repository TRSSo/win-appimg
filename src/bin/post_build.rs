use std::{
  env,
  ffi::OsStr,
  fs::{self, File},
  io::{Error, ErrorKind, Result, Seek, SeekFrom, Write, copy},
  path::{Path, PathBuf},
  process::Command,
};

use mimalloc::MiMalloc;
use sha256::try_digest;
use tempfile::tempdir;
use win_appimg::{
  dwarfs_enc, include_lang,
  payload::{PayloadData, hash_file, show_length},
};
use zerompk::to_msgpack_vec;

include_lang!();

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// WinFsp 安装包下载地址
const WINFSP_URL: &str =
  "https://github.com/winfsp/winfsp/releases/download/v2.2B4/winfsp-2.2.26215.msi";
/// WinFsp 安装包 SHA256
const WINFSP_HASH: &str = "2ecb5c89405488a95bbd8a01875e02c48534fd37bbdfd84488f7590464d65944";
/// DwarFS 发布包下载地址
const DWARFS_URL: &str =
  "https://github.com/mhx/dwarfs/releases/download/v0.15.7/dwarfs-0.15.7-Windows-AMD64.7z";
/// DwarFS 发布包 SHA256
const DWARFS_HASH: &str = "42a22273df41026bf6eb6fd6001797213388430cae5a4b1b08dfaebacf5b1d61";
/// OverlayFS 程序下载地址
const OVLFS_URL: &str =
  "https://github.com/TRSSo/WinFsp-OverlayFS/releases/download/v0.0.1/ovlfs-v0.0.1.exe";
/// OverlayFS 程序 SHA256
const OVLFS_HASH: &str = "f28564414566b46a7e1deedbcdb099a33c9131ec2d3972a14cbf8b7ce1a916cb";

/// 安装 / 卸载脚本公共头部，切换到脚本所在目录并使用 UTF-8 代码页
const CMD_HEAD: &str = r#"@echo off
chcp 65001
cd /d "%~dp0""#;
/// 安装脚本：静默安装 WinFsp 并把附带的 exe 复制到其 `bin` 目录
const INSTALL_CMD: &str = r#"
msiexec /qr /log "%~dp0\install.log" /i "winfsp.msi" ADDLOCAL=F.Developer && (
if exist "C:\Program Files (x86)\WinFsp\bin\" (
  copy /y *.exe "C:\Program Files (x86)\WinFsp\bin\"
) else if exist "C:\Program Files\WinFsp\bin\" (
  copy /y *.exe "C:\Program Files\WinFsp\bin\"
) else (
  echo Not found WinFsp path
  exit 2
)) >> "%~dp0\install.log" 2>&1 || exit !errorlevel!"#;
/// 卸载脚本：静默卸载 WinFsp 并删除其安装目录
const UNINSTALL_CMD: &str = r#"
msiexec /qr /log "%~dp0\install.log" /x "winfsp.msi" && (
if exist "C:\Program Files (x86)\WinFsp\" (
  rd /s /q "C:\Program Files (x86)\WinFsp\"
) else if exist "C:\Program Files\WinFsp\" (
  rd /s /q "C:\Program Files\WinFsp\"
)) >> "%~dp0\install.log" 2>&1 || exit !errorlevel!"#;

/// 下载 `url` 到 `path` 并校验 SHA256
///
/// 返回 `Ok(true)` 表示文件已存在且校验通过（使用缓存），`Ok(false)` 表示本次重新下载。
/// 校验失败时删除已下载的文件并返回错误。
fn download_file(url: &str, path: &Path, hash: &str) -> Result<bool> {
  if Path::new(path).exists() {
    if hash == try_digest(path)? {
      return Ok(true);
    } else {
      fs::remove_file(path)?
    }
  }

  println!("--> {}: {} {}", m!(DownloadFile), path.display(), url);
  match Command::new("curl")
    .args(["-fLo", &path.to_string_lossy(), url])
    .status()
  {
    Ok(s) if s.success() => {}
    Ok(s) => {
      return Err(Error::new(
        ErrorKind::ConnectionRefused,
        m!(DownloadError, s),
      ));
    }
    Err(e) => {
      return Err(Error::new(e.kind(), m!(StartCurlError, e)));
    }
  }

  if hash != try_digest(path)? {
    return Err(Error::new(
      ErrorKind::ConnectionAborted,
      m!(HashVerifyFailed),
    ));
  }
  Ok(false)
}

/// 下载并打包依赖到 `path/depend.dwarfs`，返回该镜像路径
///
/// 所有下载文件都已缓存且镜像已存在时直接复用，否则重新下载、解压并打包。
fn get_depend(path: &Path) -> Result<PathBuf> {
  let output_path = path.join("depend.dwarfs");
  println!("=> {}: {}", m!(BuildFile), output_path.display());
  let temp_path = tempdir()?;

  let winfsp_path = path.join("winfsp.msi");
  let dwarfs_path = path.join("dwarfs.7z");
  let ovlfs_path = path.join("ovlfs.exe");
  // download_file 返回 true 表示命中缓存；三者全部命中且镜像已存在才能跳过打包
  if ![
    download_file(WINFSP_URL, &winfsp_path, WINFSP_HASH)?,
    download_file(DWARFS_URL, &dwarfs_path, DWARFS_HASH)?,
    download_file(OVLFS_URL, &ovlfs_path, OVLFS_HASH)?,
  ]
  .contains(&false)
    && output_path.exists()
  {
    println!("--> {}", m!(UseCache));
    return Ok(output_path);
  }

  println!("--> {}: {}", m!(ExtractFile), dwarfs_path.display());
  let dwarfs_temp_path = temp_path.path().join("dwarfs");
  match Command::new("7z")
    .args([
      "e",
      &dwarfs_path.to_string_lossy(),
      &format!("-o{}", dwarfs_temp_path.display()),
      "-y",
    ])
    .status()
  {
    Ok(s) if s.success() => {}
    Ok(s) => {
      return Err(Error::new(
        ErrorKind::ConnectionRefused,
        m!(ExtractError, s),
      ));
    }
    Err(e) => {
      return Err(Error::new(e.kind(), m!(Start7zError, e)));
    }
  }

  let depend_path = temp_path.path().join("depend");
  fs::create_dir_all(&depend_path)?;
  fs::copy(winfsp_path, depend_path.join("winfsp.msi"))?;
  fs::rename(
    dwarfs_temp_path.join("dwarfs.exe"),
    depend_path.join("dwarfs.exe"),
  )?;
  fs::copy(ovlfs_path, depend_path.join("ovlfs.exe"))?;
  fs::write(
    depend_path.join("install.cmd"),
    format!("{CMD_HEAD}{INSTALL_CMD}"),
  )?;
  fs::write(
    depend_path.join("uninstall.cmd"),
    format!("{CMD_HEAD}{UNINSTALL_CMD}"),
  )?;

  println!("--> {}: {}", m!(PackFile), depend_path.display());
  dwarfs_enc::Packer::from(
    &depend_path,
    File::create(&output_path)?,
    dwarfs_enc::CompressParam::Zstd(22),
  )?
  .finish()
  .inspect_err(|_| {
    let _ = fs::remove_file(&output_path);
  })?;

  Ok(output_path)
}

/// 把依赖与启动程序追加到 `source_path`，并把信息偏移写回其 `.payload` 段
///
/// 处理完成后会删除原始的 `source_path`，输出文件名为 `win-appimg-<原名>`。
#[cfg(feature = "cli")]
fn handle(
  source_path: &Path,
  launcher_file: &mut File,
  launcher_length: u64,
  launcher_hash: u64,
  depend_file: &mut File,
  depend_length: u64,
  depend_hash: u64,
) -> Result<()> {
  use win_appimg::payload::Payload;

  let output_path = source_path.with_file_name(format!(
    "{}-{}",
    env!("CARGO_PKG_NAME"),
    source_path.file_name().unwrap_or(OsStr::new("")).display()
  ));
  println!("=> {}: {}", m!(BuildFile), output_path.display());
  let mut source_file = File::open(source_path)?;
  let source_length = source_file.metadata()?.len();

  #[cfg(windows)]
  let source_payload = win_appimg::payload::find_offset(
    &editpe::Image::parse_reader(&mut source_file)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(ReadProgramError, e)))?,
  )?;
  #[cfg(not(windows))]
  let source_payload = {
    use std::io::Read;
    use win_appimg::payload::PAYLOAD_PLACEHOLDER;
    let mut data = Vec::new();
    source_file.read_to_end(&mut data)?;
    data
      .windows(PAYLOAD_PLACEHOLDER.len())
      .position(|x| x == PAYLOAD_PLACEHOLDER)
      .map_or(
        Err(Error::new(ErrorKind::NotFound, m!(PayloadOffsetNotFound))),
        |pos| Ok(pos as u64),
      )
  }?;

  let mut file = File::create(&output_path)?;
  source_file.seek(SeekFrom::Start(0))?;
  copy(&mut source_file, &mut file)?;

  file.seek(SeekFrom::End(0))?;
  println!("--> {} {}", m!(WriteDepend), show_length(depend_length));
  depend_file.seek(SeekFrom::Start(0))?;
  copy(depend_file, &mut file)?;

  println!("--> {} {}", m!(WriteProgram), show_length(launcher_length));
  launcher_file.seek(SeekFrom::Start(0))?;
  copy(launcher_file, &mut file)?;

  let payload = Payload {
    depend: PayloadData {
      offset: source_length,
      length: depend_length,
      hash: depend_hash,
    },
    launcher: PayloadData {
      offset: source_length + depend_length,
      length: launcher_length,
      hash: launcher_hash,
    },
  };
  let payload_bytes = to_msgpack_vec(&payload)
    .map_err(|e| Error::new(ErrorKind::InvalidData, m!(InfoSerializeError, e)))?;
  let payload_offset = payload.launcher.offset + payload.launcher.length;
  let payload_length = payload_bytes.len() as u64;
  println!(
    "--> {} {}: {:?}",
    m!(WriteInfo),
    show_length(payload_length),
    payload
  );
  file.write_all(&payload_bytes)?;

  println!(
    "--> {} {}: {}",
    m!(WriteInfoOffset),
    source_payload,
    payload_offset
  );
  file.seek(SeekFrom::Start(source_payload))?;
  file.write_all(&payload_offset.to_le_bytes())?;
  file.write_all(&payload_length.to_le_bytes())?;

  file.flush()?;
  fs::remove_file(source_path)?;

  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = file.metadata()?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    file.set_permissions(perms)?;
  }

  println!(
    "=> {} {}: {}",
    m!(ProcessComplete),
    show_length(file.metadata()?.len()),
    output_path.display()
  );
  Ok(())
}

/// 构建 `cli` 与 `depend`：把依赖包、启动程序追加到 `cli.exe` 之后
///
/// 依赖包缺失时会先下载、解压并打包，产物名为 `win-appimg-<原名>`。
fn main() -> Result<()> {
  init_lang();
  let current_exe = env::current_exe()?;
  let output_path = current_exe.parent().unwrap_or(Path::new("."));

  #[cfg(windows)]
  let (launcher_path, cli_path) = (
    output_path.join("launcher.exe"),
    output_path.join("cli.exe"),
  );
  #[cfg(not(windows))]
  let (launcher_path, cli_path) = (
    env::args()
      .nth(1)
      .map_or(output_path.join("launcher.exe"), PathBuf::from),
    output_path.join("cli"),
  );

  for i in [&launcher_path, &cli_path] {
    if !i.exists() {
      return Err(Error::new(
        ErrorKind::NotFound,
        m!(NotFoundFile, i.display()),
      ));
    }
  }

  #[cfg(any(feature = "cli", feature = "depend"))]
  let (mut depend_file, depend_length, depend_hash) = {
    let depend_path = get_depend(output_path.parent().unwrap_or(Path::new(".")))?;
    let mut depend_file = File::open(depend_path)?;
    let depend_length = depend_file.metadata()?.len();
    let depend_hash = hash_file(&mut depend_file)?;
    (depend_file, depend_length, depend_hash)
  };

  #[cfg(feature = "cli")]
  {
    let mut launcher_file = File::open(&launcher_path)?;
    let launcher_length = launcher_file.metadata()?.len();
    let launcher_hash = hash_file(&mut launcher_file)?;

    handle(
      &cli_path,
      &mut launcher_file,
      launcher_length,
      launcher_hash,
      &mut depend_file,
      depend_length,
      depend_hash,
    )?;
  }

  #[cfg(feature = "depend")]
  handle_depend(
    &output_path.join("depend.exe"),
    &mut depend_file,
    depend_length,
    depend_hash,
  )?;
  Ok(())
}

/// 把依赖包追加到 `source_path`，并把信息偏移写回其 `.payload` 段
///
/// 与 [`handle`] 不同，这里只追加依赖包，输出文件名为 `win-appimg-<原名>`。
#[cfg(feature = "depend")]
fn handle_depend(
  source_path: &Path,
  depend_file: &mut File,
  depend_length: u64,
  depend_hash: u64,
) -> Result<()> {
  let output_path = source_path.with_file_name(format!(
    "{}-{}",
    env!("CARGO_PKG_NAME"),
    source_path.file_name().unwrap_or(OsStr::new("")).display()
  ));
  println!("=> {}: {}", m!(BuildFile), output_path.display());

  let mut source_file = File::open(source_path)?;
  let source_length = source_file.metadata()?.len();

  let source_payload = win_appimg::payload::find_offset(
    &editpe::Image::parse_reader(&mut source_file)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(ReadProgramError, e)))?,
  )?;

  let mut file = File::create(&output_path)?;
  source_file.seek(SeekFrom::Start(0))?;
  copy(&mut source_file, &mut file)?;

  println!("--> {} {}", m!(WriteDepend), show_length(depend_length));
  depend_file.seek(SeekFrom::Start(0))?;
  copy(depend_file, &mut file)?;

  let payload = PayloadData {
    offset: source_length,
    length: depend_length,
    hash: depend_hash,
  };
  let payload_bytes = to_msgpack_vec(&payload)
    .map_err(|e| Error::new(ErrorKind::InvalidData, m!(InfoSerializeError, e)))?;
  let payload_offset = source_length + payload.length;
  let payload_length = payload_bytes.len() as u64;
  println!(
    "--> {} {}: {:?}",
    m!(WriteInfo),
    show_length(payload_length),
    payload
  );
  file.write_all(&payload_bytes)?;

  println!(
    "--> {} {}: {}",
    m!(WriteInfoOffset),
    source_payload,
    payload_offset
  );
  file.seek(SeekFrom::Start(source_payload))?;
  file.write_all(&payload_offset.to_le_bytes())?;
  file.write_all(&payload_length.to_le_bytes())?;

  file.flush()?;

  println!(
    "=> {} {}: {}",
    m!(ProcessComplete),
    show_length(file.metadata()?.len()),
    output_path.display()
  );
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hash_constants_are_64_hex_chars() {
    for (url, hash) in [
      (WINFSP_URL, WINFSP_HASH),
      (DWARFS_URL, DWARFS_HASH),
      (OVLFS_URL, OVLFS_HASH),
    ] {
      assert!(!url.is_empty(), "下载地址不能为空");
      assert_eq!(hash.len(), 64, "SHA256 应为 64 个十六进制字符: {url}");
      assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "SHA256 含非法字符: {hash}"
      );
    }
  }
}
