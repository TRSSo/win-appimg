use std::{
  env::current_exe,
  ffi::OsStr,
  fs::{self, File},
  io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write, copy},
  path::PathBuf,
  process::{Command, Stdio},
};

use clap::Parser;
use editpe::{
  Image, ResourceDirectory, ResourceEntryName, VersionStringTable,
  constants::{RT_GROUP_ICON, RT_ICON, RT_VERSION},
  types::VersionU32,
};
use win_appimg::{
  PathConvert, dwarfs_enc, include_lang,
  payload::{
    LauncherPayload, Payload, PayloadData, WritableType, find_pe_offset, get_payload, hash_file,
    show_length,
  },
};
use zerompk::to_msgpack_vec;

include_lang!();

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(
  name = env!("CARGO_PKG_NAME"),
  version = concat!("v", env!("CARGO_PKG_VERSION")),
  about = m!(About),
  long_about = m!(LongAbout, env!("CARGO_PKG_NAME"))
)]
/// `cli` 的命令行参数
struct Args {
  /// 源文件路径
  #[arg(help = m!(Source))]
  source: PathBuf,
  /// 输出文件路径
  #[arg(help = m!(Output))]
  output: PathBuf,
  /// 启动命令，参数中的 `{}` 会在运行时替换为应用根目录
  #[arg(last = true, help = m!(Command,))]
  command: Vec<String>,
  /// 工作目录
  #[arg(short, long, value_name = "DIR", help = m!(Workdir))]
  workdir: Option<PathBuf>,
  /// 压缩等级
  #[arg(
    short,
    long,
    value_name = "1-22",
    default_value_t = 22,
    value_parser = clap::value_parser!(i32).range(1..=22),
    help = m!(CompressLevel)
  )]
  compress_level: i32,
  /// 启动外部程序
  #[arg(short, long, help = m!(External))]
  external: bool,

  /// 不打包依赖
  #[arg(long, help = m!(NoDepend))]
  no_depend: bool,
  /// 显示控制台
  #[arg(long, help = m!(ShowConsole))]
  show_console: bool,
  /// 请求管理员权限
  #[arg(long, help = m!(ReqAdmin))]
  req_admin: bool,

  /// 复制启动程序信息
  #[arg(long, help = m!(CopyInfo))]
  copy_info: bool,
  /// 复制启动程序图标
  #[arg(long, help = m!(CopyIcon))]
  copy_icon: bool,
  /// 设置信息
  #[arg(long, value_name = "KEY=VALUE", help = m!(SetInfo))]
  set_info: Option<Vec<String>>,
  /// 设置图标
  #[arg(long, value_name = "FILE", help = m!(SetIcon))]
  set_icon: Option<String>,
  /// 设置清单
  #[arg(long, value_name = "FILE", hide_short_help = true, help = m!(SetManifest))]
  set_manifest: Option<PathBuf>,

  /// 使用 mkdwarfs 构建镜像
  #[arg(long, help = m!(UseMkdwarfs))]
  use_mkdwarfs: bool,

  /// 判断为启动成功时间
  #[arg(
    long,
    value_name = "SECOND",
    default_value_t = 0,
    hide_short_help = true,
    help = m!(StartedTime)
  )]
  started_time: u16,

  /// 保存可写层（实验性）
  #[arg(long, help = m!(Write))]
  write: bool,
  /// 临时可写层（实验性）
  #[arg(long, hide_short_help = true, help = m!(WriteTemp))]
  write_temp: bool,
  /// 内存可写层（实验性）
  #[arg(long, hide_short_help = true, help = m!(WriteMemory))]
  write_memory: bool,
}

/// 把源目录 / 源镜像打包成可自启动的 Windows 应用镜像
///
/// 流程：构建 DwarFS 镜像 → 复制内嵌的启动程序 → 写入资源与信息 →
/// 追加依赖包、镜像与启动信息 → 把信息偏移写回 `.payload` 段。
fn main() -> Result<()> {
  init_lang();
  let args = Args::parse();
  let (mut exe_path, exe_args) = if let Some((p, a)) = args.command.split_first() {
    (PathBuf::from(p), a.to_vec())
  } else {
    return Err(Error::new(ErrorKind::InvalidInput, m!(InputCommand)));
  };
  let mut exe_real_path = None;

  let mut source_file = if args.source.is_dir() {
    if !args.external {
      if !exe_path.is_absolute() && args.source.join(&exe_path).exists() {
        exe_real_path = Some(args.source.join(&exe_path))
      } else if let Ok(p) = exe_path.to_relative_limit(&args.source) {
        exe_real_path = Some(exe_path);
        exe_path = p;
      } else if exe_path.exists() {
        exe_real_path = Some(exe_path.clone());
        exe_path = exe_path.to_absolute()?;
      } else {
        return Err(Error::new(ErrorKind::NotFound, m!(NotFoundCommand)));
      }
    }

    println!("=> {}: {}", m!(BuildImage), args.source.display());
    let temp_file = tempfile::tempfile()?;
    if args.use_mkdwarfs {
      if !Command::new("mkdwarfs")
        .arg("-l")
        .arg(args.compress_level.to_string())
        .arg("-i")
        .arg(args.source.as_os_str())
        .arg("-o")
        .arg("-")
        .stdout(Stdio::from(temp_file.try_clone()?))
        .status()?
        .success()
      {
        return Err(Error::new(ErrorKind::InvalidData, m!(BuildImageFail)));
      }
    } else {
      dwarfs_enc::Packer::from(
        &args.source,
        &temp_file,
        dwarfs_enc::CompressParam::Zstd(args.compress_level),
      )?
      .finish()?;
    }
    temp_file
  } else if args.source.is_file() {
    File::open(&args.source)?
  } else {
    return Err(Error::new(ErrorKind::NotFound, m!(NotFoundSource)));
  };
  if exe_path.is_absolute() {
    println!("!-> {} {}", m!(AbsoluteCommand), exe_path.display())
  }
  if let Some(p) = &args.workdir
    && p.is_absolute()
  {
    println!("!-> {} {}", m!(AbsoluteWorkdir), p.display())
  }

  let mut file = File::open(current_exe()?)?;
  let payload = get_payload::<Payload>(&mut file)?;

  let output_path = ensure_exe_extension(args.output);

  println!("=> {}: {}", m!(BuildFile), output_path.display());
  if let Some(parent) = output_path.parent()
    && !parent.as_os_str().is_empty()
  {
    fs::create_dir_all(parent)?;
  }
  let mut output_file = File::create(&output_path)?;

  println!(
    "--> {} {}",
    m!(ReadProgram),
    show_length(payload.launcher.length)
  );
  file.seek(SeekFrom::Start(payload.launcher.offset))?;
  let mut launcher_file = file.try_clone()?.take(payload.launcher.length);
  let mut image = Image::parse_reader(&mut launcher_file)
    .map_err(|e| Error::new(ErrorKind::InvalidData, m!(ReadProgramError, e)))?;

  if args.show_console {
    /// PE 子系统 `IMAGE_SUBSYSTEM_WINDOWS_CUI`，即带控制台窗口的字符界面程序
    ///
    /// 启动程序默认编译为 GUI（`2`），`--show-console` 时改写为该值。
    const SUBSYSTEM_WINDOWS_CUI: u16 = 3;
    println!("--> {}: {SUBSYSTEM_WINDOWS_CUI}", m!(WriteSubsystem));
    image.set_subsystem(SUBSYSTEM_WINDOWS_CUI);
  }

  let mut image_resource: Option<ResourceDirectory> = None;

  if args.req_admin {
    println!("--> {}", m!(WriteReqAdmin));
    let mut resource =
      image_resource.unwrap_or_else(|| image.resource_directory().cloned().unwrap_or_default());

    let manifest = resource
      .get_manifest()
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(ReadManifestError, e)))?
      .ok_or(Error::new(ErrorKind::NotFound, m!(NotFoundManifest)))?
      .replace("asInvoker", "requireAdministrator");

    resource
      .set_manifest(&manifest)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(WriteManifestError, e)))?;
    image_resource = Some(resource)
  }

  if args.copy_info || args.copy_icon {
    let path = exe_real_path.ok_or(Error::new(ErrorKind::NotFound, m!(NotFoundCommand)))?;
    let exe_resource = Image::parse_file(&path)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(ReadCommandError, e)))?
      .resource_directory()
      .cloned()
      .ok_or(Error::new(
        ErrorKind::InvalidData,
        m!(ReadCommandResourceError),
      ))?;

    let mut resource =
      image_resource.unwrap_or_else(|| image.resource_directory().cloned().unwrap_or_default());
    let mut copy_entry = |id: u32| {
      let name = &ResourceEntryName::ID(id);
      resource.root_mut().insert(
        name,
        exe_resource
          .root()
          .get(name)
          .ok_or(Error::new(ErrorKind::NotFound, m!(ReadCommandError, name)))?
          .clone(),
      );
      Ok::<(), Error>(())
    };

    if args.copy_info {
      println!("--> {}", m!(CopyInfo));
      copy_entry(RT_VERSION as u32)?;
    }

    if args.copy_icon {
      println!("--> {}", m!(CopyIcon));
      copy_entry(RT_ICON as u32)?;
      copy_entry(RT_GROUP_ICON as u32)?;
    }
    image_resource = Some(resource)
  }

  if let Some(info) = args.set_info {
    println!("--> {}: {info:?}", m!(WriteInfo));
    let mut resource =
      image_resource.unwrap_or_else(|| image.resource_directory().cloned().unwrap_or_default());

    let mut version_info = resource
      .get_version_info()
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(ReadInfoError, e)))?
      .unwrap_or_default();
    if version_info.strings.is_empty() {
      version_info.strings.push(VersionStringTable {
        key: "040904b0".to_string(),
        ..VersionStringTable::default()
      })
    }

    for i in info {
      let (key, value) = i
        .split_once('=')
        .ok_or(Error::new(ErrorKind::InvalidInput, m!(InvalidInputInfo, i)))?;
      version_info.strings[0]
        .strings
        .insert(key.to_string(), value.to_string());
      match key {
        "FileVersion" => set_version(value, &mut version_info.info.file_version),
        "ProductVersion" => set_version(value, &mut version_info.info.product_version),
        _ => {}
      }
    }

    resource
      .set_version_info(&version_info)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(WriteInfoError, e)))?;
    image_resource = Some(resource)
  }

  if let Some(icon) = args.set_icon {
    println!("--> {}: {icon}", m!(WriteIcon));
    let mut resource =
      image_resource.unwrap_or_else(|| image.resource_directory().cloned().unwrap_or_default());
    resource
      .set_main_icon_file(&icon)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(WriteIconError, e)))?;
    image_resource = Some(resource)
  }

  if let Some(manifest) = args.set_manifest {
    println!("--> {}: {}", m!(WriteManifest), manifest.display());
    let manifest = fs::read_to_string(manifest)?;
    let mut resource =
      image_resource.unwrap_or_else(|| image.resource_directory().cloned().unwrap_or_default());
    resource
      .set_manifest(&manifest)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(WriteManifestError, e)))?;
    image_resource = Some(resource)
  }

  if let Some(resource) = image_resource {
    println!("--> {}", m!(WriteResource));
    image
      .set_resource_directory(resource)
      .map_err(|e| Error::new(ErrorKind::InvalidData, m!(WriteResourceError, e)))?;
  }

  let launcher_payload_offset = find_pe_offset(&image)?;
  image
    .write_writer(&mut output_file)
    .map_err(|e| Error::new(ErrorKind::InvalidData, m!(WriteProgramError, e)))?;
  let launcher_length = output_file.metadata()?.len();
  println!("--> {} {}", m!(WriteProgram), show_length(launcher_length));

  let mut launcher_payload = LauncherPayload {
    depend: PayloadData {
      offset: 0,
      length: 0,
      hash: 0,
    },
    image: PayloadData {
      offset: launcher_length,
      length: source_file.metadata()?.len(),
      hash: hash_file(&mut source_file)?,
    },

    exe_path: exe_path.to_string_lossy().to_string(),
    exe_args,
    exe_workdir: args.workdir.map(|s| s.to_string_lossy().to_string()),

    exe_console: args.show_console,
    exe_external: args.external,
    exe_started_time: args.started_time,

    exe_writable: if args.write {
      WritableType::Save
    } else if args.write_temp {
      WritableType::Temp
    } else if args.write_memory {
      WritableType::Memory
    } else {
      WritableType::None
    },
  };

  if !args.no_depend {
    println!(
      "--> {} {}",
      m!(WriteDepend),
      show_length(payload.depend.length)
    );

    launcher_payload.depend = PayloadData {
      offset: launcher_length,
      length: payload.depend.length,
      hash: payload.depend.hash,
    };
    launcher_payload.image.offset = launcher_payload.depend.offset + payload.depend.length;

    file.seek(SeekFrom::Start(payload.depend.offset))?;
    let mut depend_file = file.try_clone()?.take(payload.depend.length);
    copy(&mut depend_file, &mut output_file)?;
  };

  println!(
    "--> {} {}",
    m!(WriteImage),
    show_length(launcher_payload.image.length)
  );
  source_file.seek(SeekFrom::Start(0))?;
  copy(&mut source_file, &mut output_file)?;

  let payload_bytes = to_msgpack_vec(&launcher_payload)
    .map_err(|e| Error::new(ErrorKind::InvalidData, m!(InfoSerializeError, e)))?;
  let payload_offset = launcher_payload.image.offset + launcher_payload.image.length;
  let payload_length = payload_bytes.len() as u64;
  println!(
    "--> {} {}: {:?}",
    m!(WriteInfo),
    show_length(payload_length),
    launcher_payload
  );
  output_file.write_all(&payload_bytes)?;

  println!(
    "--> {} {}: {}",
    m!(WriteInfoOffset),
    launcher_payload_offset,
    payload_offset
  );
  output_file.seek(SeekFrom::Start(launcher_payload_offset))?;
  output_file.write_all(&payload_offset.to_le_bytes())?;
  output_file.write_all(&payload_length.to_le_bytes())?;

  output_file.flush()?;
  println!(
    "=> {} {}: {}",
    m!(ProcessComplete),
    show_length(output_file.metadata()?.len()),
    output_path.display()
  );
  Ok(())
}

/// 确保输出路径以 `.exe` 结尾
///
/// 没有扩展名或扩展名不是 `exe` 时追加 `.exe`，其余情况原样返回。
fn ensure_exe_extension(path: PathBuf) -> PathBuf {
  if let Some(ext) = path.extension()
    && ext.eq_ignore_ascii_case("exe")
  {
    return path;
  }
  let mut name = path.file_name().unwrap_or(OsStr::new("")).to_os_string();
  name.push(".exe");
  let mut path = path;
  path.set_file_name(name);
  path
}

/// 把 `a.b.c.d` 形式的字符串写入 PE 版本字段
///
/// 每段按 `u16` 解析，非法段按 0 处理，最多使用前 4 段。
fn set_version(string: &str, version: &mut VersionU32) {
  let mut v = [0u16; 4];
  let mut n = 0;
  for i in string.split('.') {
    v[n] = i.parse().unwrap_or(0);
    n += 1;
    if n > 3 {
      break;
    }
  }
  version.major = (v[0] as u32) << 16 | v[1] as u32;
  version.minor = (v[2] as u32) << 16 | v[3] as u32;
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 解析版本号字符串，返回 `(major, minor)`
  fn version_of(string: &str) -> (u32, u32) {
    let mut version = VersionU32::default();
    set_version(string, &mut version);
    (version.major, version.minor)
  }

  #[test]
  fn set_version_full_four_parts() {
    // a.b.c.d -> major = a << 16 | b, minor = c << 16 | d
    assert_eq!(version_of("1.2.3.4"), (0x0001_0002, 0x0003_0004));
    assert_eq!(version_of("0.0.0.0"), (0, 0));
  }

  #[test]
  fn set_version_missing_parts_default_to_zero() {
    assert_eq!(version_of("1"), (0x0001_0000, 0));
    assert_eq!(version_of("1.2"), (0x0001_0002, 0));
    assert_eq!(version_of("1.2.3"), (0x0001_0002, 0x0003_0000));
  }

  #[test]
  fn set_version_rejects_invalid_segments() {
    // 非法段按 0 处理，不会 panic
    assert_eq!(version_of(""), (0, 0));
    assert_eq!(version_of("a.b.c.d"), (0, 0));
    assert_eq!(version_of("1.x.3.4"), (0x0001_0000, 0x0003_0004));
    assert_eq!(version_of("-1.2.3.4"), (0x0000_0002, 0x0003_0004));
  }

  #[test]
  fn set_version_truncates_to_four_parts() {
    assert_eq!(version_of("1.2.3.4.5"), (0x0001_0002, 0x0003_0004));
    assert_eq!(version_of("1.2.3.4.5.6"), (0x0001_0002, 0x0003_0004));
  }

  #[test]
  fn set_version_handles_overflowing_segments() {
    // u16 溢出时按 0 处理
    assert_eq!(version_of("65536.1.2.3"), (0x0000_0001, 0x0002_0003));
    assert_eq!(version_of("70000"), (0, 0));
  }

  #[test]
  fn set_version_max_u16() {
    assert_eq!(
      version_of("65535.65535.65535.65535"),
      (0xFFFF_FFFF, 0xFFFF_FFFF)
    );
  }

  #[test]
  fn ensure_exe_extension_appends_to_bare_name() {
    assert_eq!(
      ensure_exe_extension(PathBuf::from("app")),
      PathBuf::from("app.exe")
    );
    assert_eq!(
      ensure_exe_extension(PathBuf::from("dir/app")),
      PathBuf::from("dir/app.exe")
    );
  }

  #[test]
  fn ensure_exe_extension_keeps_exe() {
    assert_eq!(
      ensure_exe_extension(PathBuf::from("app.exe")),
      PathBuf::from("app.exe")
    );
    assert_eq!(
      ensure_exe_extension(PathBuf::from("app.Exe")),
      PathBuf::from("app.Exe")
    );
    assert_eq!(
      ensure_exe_extension(PathBuf::from("app.EXE")),
      PathBuf::from("app.EXE")
    );
  }

  #[test]
  fn ensure_exe_extension_appends_to_other_extension() {
    assert_eq!(
      ensure_exe_extension(PathBuf::from("app.bin")),
      PathBuf::from("app.bin.exe")
    );
  }

  #[test]
  fn ensure_exe_extension_keeps_parent_directory() {
    assert_eq!(
      ensure_exe_extension(PathBuf::from("out/dir/app")),
      PathBuf::from("out/dir/app.exe")
    );
  }

  #[test]
  fn ensure_exe_extension_handles_degenerate_paths() {
    // 这些路径没有可替换的文件名，不应 panic，只会得到一个纯扩展名
    assert_eq!(
      ensure_exe_extension(PathBuf::from("")),
      PathBuf::from(".exe")
    );
    // 结尾的分隔符会被当作文件名的一部分剥离
    assert_eq!(
      ensure_exe_extension(PathBuf::from("out/app/")),
      PathBuf::from("out/app.exe")
    );
  }

  #[test]
  fn ensure_exe_extension_is_idempotent() {
    for name in ["app", "app.exe", "app.bin"] {
      let once = ensure_exe_extension(PathBuf::from(name));
      assert_eq!(ensure_exe_extension(once.clone()), once);
    }
  }
}
