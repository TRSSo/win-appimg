use std::{
  io::{Error, ErrorKind, Result},
  path::Path,
  ptr::null,
};

use crate::{PathConvert, include_lang, win::StringConvert};
use windows_sys::Win32::{
  Foundation::{CloseHandle, ERROR_CANCELLED, FALSE, GetLastError, WAIT_FAILED},
  System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
  UI::{
    Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
    WindowsAndMessaging::SW_HIDE,
  },
};

include_lang!();

/// 以管理员权限运行 `path`，隐藏其窗口并等待进程结束
///
/// 返回进程的退出码。
/// 用户取消 UAC 授权时返回 `ErrorKind::PermissionDenied`，
/// `ShellExecuteExW` 失败时返回 `ErrorKind::ConnectionRefused`。
pub fn run(path: &Path) -> Result<u32> {
  // 先转成绝对路径，工作目录也要基于绝对路径取，
  // 否则相对路径会被按当前进程的工作目录解析
  let abs_path = path.to_absolute()?;
  let file = abs_path.as_os_str().to_wide();
  let verb = "runas".to_wide();
  let workdir = abs_path.parent().map(|p| p.as_os_str().to_wide());

  let mut info = SHELLEXECUTEINFOW {
    cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,

    lpVerb: verb.as_ptr(),
    lpFile: file.as_ptr(),
    lpDirectory: workdir.as_ref().map_or(null(), |d| d.as_ptr()),

    // 返回进程句柄
    fMask: SEE_MASK_NOCLOSEPROCESS,
    // 隐藏控制台窗口
    nShow: SW_HIDE,
    ..Default::default()
  };

  if unsafe { ShellExecuteExW(&mut info) } == FALSE {
    match unsafe { GetLastError() } {
      ERROR_CANCELLED => {
        return Err(Error::new(ErrorKind::PermissionDenied, m!(UacCancelled)));
      }
      err => {
        return Err(Error::new(
          ErrorKind::ConnectionRefused,
          m!(ShellExecuteError, err),
        ));
      }
    }
  }

  let process = info.hProcess;
  if process.is_null() {
    return Err(Error::new(
      ErrorKind::ConnectionAborted,
      m!(ShellExecuteInvalidProcess),
    ));
  }
  // 等待失败时不能读取退出码，否则会得到 STILL_ACTIVE 之类的无意义值
  if unsafe { WaitForSingleObject(process, INFINITE) } == WAIT_FAILED {
    unsafe { CloseHandle(process) };
    return Err(Error::new(
      ErrorKind::Interrupted,
      m!(WaitProcessError, unsafe { GetLastError() }),
    ));
  }
  let mut code = 0;
  let res = unsafe { GetExitCodeProcess(process, &mut code) };
  unsafe { CloseHandle(process) };

  if res == FALSE {
    return Err(Error::new(
      ErrorKind::Interrupted,
      m!(GetExitCodeError, unsafe { GetLastError() }),
    ));
  }
  Ok(code)
}
