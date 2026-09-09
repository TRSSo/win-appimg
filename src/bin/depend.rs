#![windows_subsystem = "windows"]
use std::{env, fs::File, io::Result, path::Path, process::ExitCode};

use mimalloc::MiMalloc;
use tempfile::tempdir;
use win_appimg::{
  include_lang, ok, option,
  payload::{PayloadData, get_payload},
  tempname,
  win::{
    depend::{check_depend, extract_depend, install_depend, uninstall_depend},
    dialog,
  },
};

include_lang!();

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// 显示依赖管理窗口，返回 `true` 表示继续显示窗口（`false` 表示用户退出）
fn window(depend_path: &Path) -> bool {
  let (status, content) = match check_depend(true) {
    Ok(_) => (m!(StatusInstalled), "".to_string()),
    Err(e) => (m!(StatusNotInstalled), m!(DetailInfo, e)),
  };
  match option!(
    dialog::Icon::Info,
    &m!(
      DependManager,
      env!("CARGO_PKG_NAME"),
      env!("CARGO_PKG_VERSION")
    ),
    status,
    &content,
    &[m!(Install), m!(Uninstall)]
  ) {
    Some(0) => {
      install_depend(depend_path);
    }
    Some(1) => {
      uninstall_depend(depend_path);
    }
    _ => return false,
  }
  true
}

/// 读取内嵌依赖包并循环显示依赖管理窗口，直到用户退出
fn launch() -> Result<()> {
  let path = env::current_exe()?;
  let mut file = File::open(&path)?;
  let payload = get_payload::<PayloadData>(&file)?;
  let tempdir = tempdir()?;
  let depend_path = tempdir.path().join(tempname(6));
  match extract_depend(&path, &mut file, &payload, &depend_path) {
    Some(true) => while window(&depend_path) {},
    Some(false) => unreachable!(),
    None => {}
  }
  Ok(())
}

/// 显示错误对话框，并把退出码转成 [`ExitCode`]
fn main() -> ExitCode {
  init_lang();
  match launch() {
    Ok(_) => ExitCode::SUCCESS,
    Err(err) => {
      ok!(
        dialog::Icon::Error,
        m!(Error),
        &err.to_string(),
        &format!("{err:?}")
      );
      ExitCode::FAILURE
    }
  }
}
