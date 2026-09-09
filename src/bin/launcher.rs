#![windows_subsystem = "windows"]
use std::{env, fs::File, io::Result};

use mimalloc::MiMalloc;
use tempfile::tempdir;
use win_appimg::{
  include_lang, ok,
  payload::{LauncherPayload, get_payload},
  win::{dialog, launcher::Launcher},
};
use windows_sys::Win32::System::Threading::ExitProcess;

include_lang!();

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// 读取内嵌启动信息并启动应用，返回应用的退出码
fn launch() -> Result<u32> {
  let path = env::current_exe()?;
  let mut file = File::open(&path)?;
  let payload = get_payload::<LauncherPayload>(&file)?;
  let tempdir = tempdir()?;
  let mut launcher = Launcher::from(&path, &mut file, &payload, &tempdir);
  Ok(launcher.start())
}

/// 读取内嵌启动信息启动应用，失败时弹窗显示错误，并以退出码结束进程
fn main() {
  init_lang();
  match launch() {
    Ok(code) => unsafe { ExitProcess(code) },
    Err(err) => {
      ok!(
        dialog::Icon::Error,
        m!(Error),
        &err.to_string(),
        &format!("{err:?}")
      );
      unsafe { ExitProcess(u32::MAX) };
    }
  }
}
