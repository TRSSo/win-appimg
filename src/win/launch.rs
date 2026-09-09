use std::{
  fmt::Display,
  path::PathBuf,
  ptr::{null, null_mut},
  sync::{Arc, Mutex, OnceLock, Weak},
  time::Instant,
};

use crate::{include_lang, win::StringConvert};
use windows_sys::{
  Win32::{
    Foundation::{
      CloseHandle, ERROR_OPERATION_ABORTED, FALSE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
      TRUE, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
    },
    System::{
      Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
        GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
      },
      IO::{
        CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED, PostQueuedCompletionStatus,
      },
      JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_LIMIT_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectAssociateCompletionPortInformation,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
      },
      SystemServices::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO,
      Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW,
        GetExitCodeProcess, INFINITE, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
        TerminateProcess, WaitForSingleObject,
      },
    },
  },
  core::BOOL,
};

include_lang!();

/// 启动器运行期间的错误
pub enum LauncherError {
  /// Windows API 调用失败，保存 `GetLastError` 的返回值
  Win32(WIN32_ERROR),
  /// 应用进程创建失败
  AppStart(WIN32_ERROR),
  /// 应用在守护期（`exe_started_time` 秒）内退出，保存守护时长与退出码
  AppExited(u16, u32),
  /// 依赖进程创建失败，保存依赖名与错误码
  DependStart(String, WIN32_ERROR),
  /// 依赖进程退出，保存依赖名与退出码
  DependExited(String, u32),
  /// 依赖在限定时间内没有就绪，保存依赖名与其挂载目标
  DependTimeout(String, PathBuf),
  /// 收到 Ctrl+C / 关闭等停止事件
  Stopped,
}

impl LauncherError {
  /// 返回可作为进程退出码的值
  ///
  /// 0 表示成功，因此会把 0 映射为 1；[`LauncherError::Stopped`] 为 2，
  /// [`LauncherError::DependTimeout`] 为 3，其余情况透传 Windows 错误码或退出码。
  pub fn code(&self) -> u32 {
    match self {
      LauncherError::Win32(code)
      | LauncherError::DependStart(_, code)
      | LauncherError::DependExited(_, code)
      | LauncherError::AppStart(code)
      | LauncherError::AppExited(_, code) => {
        if *code == 0 {
          1
        } else {
          *code
        }
      }
      LauncherError::Stopped => 2,
      LauncherError::DependTimeout { .. } => 3,
    }
  }
}

impl Display for LauncherError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      LauncherError::Win32(code) => write!(f, "{}", m!(Win32Error, code)),
      LauncherError::AppStart(code) => write!(f, "{}", m!(AppStartError, code)),
      LauncherError::AppExited(time, code) => {
        write!(f, "{}", m!(AppExited, time))?;
        if *code != 0 {
          write!(f, "{}", m!(ExitCodeSuffix, code))?
        }
        Ok(())
      }
      LauncherError::DependStart(name, code) => {
        write!(f, "{}", m!(DependStartError, name, code))
      }
      LauncherError::DependExited(name, code) => {
        write!(f, "{}", m!(DependExited, name))?;
        if *code != 0 {
          write!(f, "{}", m!(ExitCodeSuffix, code))?
        }
        Ok(())
      }
      LauncherError::DependTimeout(name, target) => {
        write!(f, "{}", m!(DependStartupTimeout, name, target.display()))
      }
      LauncherError::Stopped => write!(f, "{}", m!(LauncherStopped)),
    }
  }
}

/// Windows 句柄包装，析构时自动关闭句柄
///
/// 用 `WinHandle(null_mut())` 覆盖旧值即可提前关闭句柄。
struct WinHandle(HANDLE);
// SAFETY: `HANDLE` 只是一个内核对象编号，本身不指向进程内存，
// 跨线程传递与共享是安全的；本类型独占句柄所有权，不会被重复关闭。
unsafe impl Sync for WinHandle {}
unsafe impl Send for WinHandle {}

/// 释放句柄，空指针会被跳过
impl Drop for WinHandle {
  fn drop(&mut self) {
    if !self.0.is_null() {
      unsafe {
        CloseHandle(self.0);
      }
    }
  }
}

/// 全局停止事件，控制台 Ctrl+C / 关闭控制台等会触发
const STOP_COMPLETION_KEY: usize = usize::MAX;
/// 当前 [`Waiter`] 的 IOCP 句柄，供控制台事件回调投递停止事件
///
/// 保存弱引用，避免 [`Waiter`] 销毁后句柄仍被持有。
static IOCP_HANDLE: OnceLock<Mutex<Weak<WinHandle>>> = OnceLock::new();

/// 控制台事件回调，把 Ctrl+C / 关闭等事件转为 IOCP 上的停止通知
///
/// 返回 `TRUE` 表示事件已处理；尚未注册 IOCP 时返回 `FALSE`，交给系统默认处理。
extern "system" fn console_ctrl_handler(ctrl_type: u32) -> BOOL {
  match ctrl_type {
    CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
    | CTRL_SHUTDOWN_EVENT => {
      if let Some(iocp) = IOCP_HANDLE.get().and_then(|i| {
        i.lock()
          .unwrap_or_else(|e| {
            #[cfg(debug_assertions)]
            eprintln!("警告: IOCP_HANDLE poisoned");
            e.into_inner()
          })
          .upgrade()
      }) {
        unsafe {
          PostQueuedCompletionStatus(
            iocp.0,
            JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO,
            STOP_COMPLETION_KEY,
            null_mut(),
          )
        }
      } else {
        FALSE
      }
    }
    _ => FALSE,
  }
}

/// 待启动的程序
#[cfg_attr(debug_assertions, derive(Debug))]
pub struct Program {
  /// 程序路径
  pub path: String,
  /// 命令行参数
  pub args: Vec<String>,
  /// 工作目录，`None` 表示继承当前进程的工作目录
  pub workdir: Option<String>,
}

/// 一个需要先于应用启动的依赖程序
pub struct Depend {
  /// 依赖名，用于错误信息
  pub name: String,
  /// 就绪标志：该路径出现即认为依赖启动完成
  pub target: PathBuf,
  /// 依赖程序
  pub program: Program,
}

/// 一个已启动并加入 Job 的子进程
pub struct Child {
  /// 进程名，用于错误信息
  pub name: String,
  /// 进程句柄
  process: WinHandle,
  /// 主线程句柄，仅需在创建后关闭，故标记为未使用
  #[allow(unused)]
  thread: WinHandle,
  /// 所属 Job 句柄，关闭时会终止其中的所有进程
  job: WinHandle,
  /// 进程 ID，仅控制台进程记录，用于发送 Ctrl+Break
  pid: Option<u32>,
}

/// 停止子进程：控制台进程优先发送 Ctrl+Break 温和退出，失败则关闭 Job 强制终止
///
/// 关闭 Job 会终止其中的所有进程，之后最多再等待 3 秒让进程退出。
impl Drop for Child {
  fn drop(&mut self) {
    unsafe {
      // 对控制台应用尝试温和退出，失败则强制关闭
      if let Some(pid) = self.pid
        && GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) == TRUE
        && WaitForSingleObject(self.process.0, 3000) == WAIT_OBJECT_0
      {
        return;
      }
      self.job = WinHandle(null_mut());
      WaitForSingleObject(self.process.0, 3000);
    }
  }
}

impl Child {
  /// 读取进程退出码，进程仍在运行或查询失败时返回 `None`
  pub fn exit_code(&self) -> Option<u32> {
    unsafe {
      let mut code = 0u32;
      if GetExitCodeProcess(self.process.0, &mut code) != 0 {
        Some(code)
      } else {
        None
      }
    }
  }
}

/// 按 Windows 命令行规则为单个参数加引号，返回不含结尾 NUL 的 UTF-16 码元
///
/// 仅在参数包含空格、制表符或引号时加引号；反斜杠只在其后紧跟引号时才是转义符。
fn quote_arg(arg: &str) -> Vec<u16> {
  // 空参数必须变成 ""
  if arg.is_empty() {
    return vec!['"' as u16, '"' as u16];
  }
  // 是否需要加引号
  if !arg.chars().any(|c| matches!(c, ' ' | '\t' | '"')) {
    return arg.encode_utf16().collect();
  }

  let mut out = Vec::with_capacity(arg.len() + 2);
  out.push('"' as u16);
  let mut bs = 0usize;

  const BS: u16 = '\\' as u16;
  const QUOTE: u16 = '"' as u16;

  for c in arg.encode_utf16() {
    match c {
      BS => {
        bs += 1;
      }
      QUOTE => {
        // 在引号前：
        // 2n + 1 个反斜杠 + 引号，表示字面量引号
        for _ in 0..(2 * bs + 1) {
          out.push('\\' as u16);
        }
        out.push('"' as u16);
        bs = 0;
      }
      _ => {
        for _ in 0..bs {
          out.push('\\' as u16);
        }
        out.push(c);
        bs = 0;
      }
    }
  }

  // 如果参数结尾有反斜杠，并且整个参数被引号包住，
  // 结尾反斜杠必须加倍，否则会转义结束引号。
  for _ in 0..(2 * bs) {
    out.push('\\' as u16);
  }
  out.push('"' as u16);
  out
}

/// 拼装 `CreateProcessW` 需要的完整命令行，返回以 NUL 结尾的 UTF-16 码元
///
/// 程序路径与各参数都会独立加引号后以空格分隔。
pub fn cmd_wide(path: &str, args: &[String]) -> Vec<u16> {
  let mut cmd = quote_arg(path);
  for arg in args {
    cmd.push(b' ' as u16);
    cmd.extend(quote_arg(arg));
  }
  cmd.push(0);
  cmd
}

/// 等待 `dep` 就绪，最多轮询 1000 次、每次 100 毫秒（约 100 秒）
///
/// 每轮先检查 `dep.target`，挂载点已出现时立即返回，不额外等待一个轮询间隔。
///
/// 期间依赖进程退出会返回 [`LauncherError::DependExited`]，
/// 超时仍未出现 `dep.target` 则返回 [`LauncherError::DependTimeout`]。
fn depend_wait(waiter: &Waiter, dep: &Depend) -> Result<(), LauncherError> {
  for _ in 0..1000 {
    match waiter.wait_timeout(100) {
      Ok((child, _)) => {
        return Err(LauncherError::DependExited(
          child.name.clone(),
          child.exit_code().unwrap_or(u32::MAX),
        ));
      }
      Err(WAIT_TIMEOUT) => {}
      Err(ERROR_OPERATION_ABORTED) => return Err(LauncherError::Stopped),
      Err(code) => return Err(LauncherError::Win32(code)),
    }
    // 文件夹存在，依赖启动成功（经过实测确定第一轮循环开始时文件夹不存在，所以写在结尾）
    if dep.target.is_dir() {
      return Ok(());
    }
  }
  Err(LauncherError::DependTimeout(
    dep.name.clone(),
    dep.target.clone(),
  ))
}

/// 多 Job 等待器，用于同时等待多个子进程组退出
///
/// 每个子进程都被分配到独立的 Job，Job 与内部 IOCP 关联，
/// 组内所有进程退出时 IOCP 会收到通知。
/// 销毁时按启动顺序的反方向停止所有子进程。
pub struct Waiter {
  /// 所有 Job 共用的 IOCP 句柄，控制台事件回调通过弱引用向它投递停止事件
  iocp: Arc<WinHandle>,
  /// 已启动的子进程，下标即其 Job 的完成键
  childs: Vec<Child>,
}

/// 按启动顺序的反方向停止所有子进程，保证后启动的依赖先退出
impl Drop for Waiter {
  fn drop(&mut self) {
    while self.childs.pop().is_some() {}
  }
}

impl Waiter {
  /// 创建一个新的多 Job 等待器
  ///
  /// 创建 IOCP 失败时返回 `GetLastError` 的返回值。
  pub fn new() -> Result<Self, WIN32_ERROR> {
    // 创建一个不与任何文件关联的独立 IOCP
    let iocp = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 0) };
    if iocp.is_null() {
      return Err(unsafe { GetLastError() });
    }

    Ok(Self {
      iocp: Arc::new(WinHandle(iocp)),
      childs: Vec::new(),
    })
  }

  /// 无限等待，直到任意一个 Job 中的所有进程退出
  ///
  /// 返回该 Job 对应的子进程与其在 `childs` 中的下标，
  /// 错误码含义见 [`Waiter::wait_timeout`]（不会返回 `WAIT_TIMEOUT`）。
  pub fn wait(&self) -> Result<(&Child, usize), WIN32_ERROR> {
    self.wait_timeout(u32::MAX)
  }

  /// 带超时的等待，直到任意一个 Job 中的所有进程退出
  ///
  /// `timeout_ms` 为 `u32::MAX` 表示无限等待。
  ///
  /// 返回该 Job 对应的子进程与其在 `childs` 中的下标；
  /// 超时返回 `WAIT_TIMEOUT`，收到停止事件返回 `ERROR_OPERATION_ABORTED`，
  /// 其余情况返回 `GetLastError` 的返回值。
  pub fn wait_timeout(&self, timeout_ms: u32) -> Result<(&Child, usize), WIN32_ERROR> {
    let start = Instant::now();
    loop {
      // 计算当前循环剩余的超时时间
      let ms = if timeout_ms == u32::MAX {
        INFINITE
      } else {
        let elapsed = start.elapsed().as_millis() as u32;
        if elapsed >= timeout_ms {
          return Err(WAIT_TIMEOUT);
        }
        timeout_ms - elapsed
      };

      let mut code = 0;
      let mut key = 0;
      let mut overlapped: *mut OVERLAPPED = null_mut();

      if unsafe { GetQueuedCompletionStatus(self.iocp.0, &mut code, &mut key, &mut overlapped, ms) }
        == FALSE
      {
        return Err(unsafe { GetLastError() });
      }

      if code == JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO {
        if key == STOP_COMPLETION_KEY {
          return Err(ERROR_OPERATION_ABORTED);
        } else if let Some(job) = self.childs.get(key) {
          return Ok((job, key));
        }
      }
    }
  }

  /// 启动一个进程，并加入到 Waiter 中
  ///
  /// - `name`：进程名，仅用于错误信息
  /// - `program`：待启动的程序
  /// - `console`：为 `true` 时继承控制台并新建进程组（可用 Ctrl+Break 停止），
  ///   否则以 `CREATE_NO_WINDOW` 隐藏窗口
  ///
  /// 进程先挂起创建，加入 Job 并关联 IOCP 后再恢复运行，
  /// 因此中途失败不会留下游离进程。失败时返回 `GetLastError` 的返回值，
  /// 且不会把该进程加入 `childs`。
  pub fn spawn(&mut self, name: &str, program: &Program, console: bool) -> Result<(), WIN32_ERROR> {
    #[cfg(debug_assertions)]
    println!("{name} {program:?}");

    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
      return Err(unsafe { GetLastError() });
    }
    let job = WinHandle(job);

    let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
      BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
        LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        ..Default::default()
      },
      ..Default::default()
    };
    if unsafe {
      SetInformationJobObject(
        job.0,
        JobObjectExtendedLimitInformation,
        &info as *const _ as _,
        size_of_val(&info) as u32,
      )
    } == FALSE
    {
      return Err(unsafe { GetLastError() });
    }

    let mut cmd_w = cmd_wide(&program.path, &program.args);
    let workdir_w = program.workdir.as_ref().map(|p| p.to_wide());

    let si = STARTUPINFOW {
      cb: size_of::<STARTUPINFOW>() as u32,
      ..Default::default()
    };
    let mut pi: PROCESS_INFORMATION = Default::default();

    // CREATE_SUSPENDED：先挂起，把进程挂到 Job 上
    // CREATE_NEW_PROCESS_GROUP：用于 GenerateConsoleCtrlEvent
    // CREATE_NO_WINDOW 不显示控制台，不可用 GenerateConsoleCtrlEvent
    let flags = CREATE_SUSPENDED
      | if console {
        CREATE_NEW_PROCESS_GROUP
      } else {
        CREATE_NO_WINDOW
      };

    if unsafe {
      CreateProcessW(
        null(),
        cmd_w.as_mut_ptr(),
        null(),
        null(),
        FALSE,
        flags,
        null(),
        workdir_w.as_ref().map_or(null(), |v| v.as_ptr()),
        &si,
        &mut pi,
      )
    } == FALSE
    {
      return Err(unsafe { GetLastError() });
    }
    let process = WinHandle(pi.hProcess);
    let thread = WinHandle(pi.hThread);

    // 记录完成键，之后用于区分是哪个子进程组退出
    unsafe {
      let port = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: self.childs.len() as _,
        CompletionPort: self.iocp.0,
      };
      if AssignProcessToJobObject(job.0, pi.hProcess) == FALSE
        || SetInformationJobObject(
          job.0,
          JobObjectAssociateCompletionPortInformation,
          &port as *const _ as _,
          size_of_val(&port) as u32,
        ) == FALSE
        || ResumeThread(pi.hThread) == u32::MAX
      {
        let code = GetLastError();
        TerminateProcess(pi.hProcess, 1);
        return Err(code);
      }
    }
    self.childs.push(Child {
      name: name.to_string(),
      process,
      thread,
      job,
      pid: if console { Some(pi.dwProcessId) } else { None },
    });
    Ok(())
  }
}

/// 依次启动所有依赖，再启动应用，并等待应用结束
///
/// - `depend`：按数组顺序启动，每个都需就绪后才启动下一个
/// - `started_time`：非 0 时，应用在该秒数内退出视为启动失败
/// - `console`：为 `true` 时子进程继承控制台（可用 Ctrl+Break 停止），否则隐藏窗口
///
/// 返回应用的退出码；依赖先于应用退出、收到停止事件或其他错误时返回 [`LauncherError`]。
pub fn run(
  app: &Program,
  depend: &[Depend],
  started_time: u16,
  console: bool,
) -> Result<u32, LauncherError> {
  let mut waiter = Waiter::new().map_err(LauncherError::Win32)?;

  {
    let mut guard = IOCP_HANDLE
      .get_or_init(|| {
        if unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), TRUE) } == FALSE {
          #[cfg(debug_assertions)]
          eprintln!("警告: SetConsoleCtrlHandler 失败 {}", unsafe {
            GetLastError()
          });
        }
        Mutex::new(Weak::new())
      })
      .lock()
      .unwrap_or_else(|e| {
        #[cfg(debug_assertions)]
        eprintln!("警告: IOCP_HANDLE poisoned");
        e.into_inner()
      });
    #[cfg(debug_assertions)]
    if let Some(iocp) = guard.upgrade() {
      eprintln!("警告: 已存在 IOCP_HANDLE {:?} 将被覆盖", iocp.0);
    }
    *guard = Arc::downgrade(&waiter.iocp);
  }

  // 启动依赖进程
  for dep in depend {
    match waiter.spawn(&dep.name, &dep.program, console) {
      Ok(c) => c,
      Err(code) => {
        return Err(LauncherError::DependStart(dep.name.clone(), code));
      }
    };
    depend_wait(&waiter, dep)?
  }

  // 启动应用进程，并记录其完成键用于区分是应用还是依赖退出
  let app_index = {
    match waiter.spawn("App", app, console) {
      Ok(()) => {}
      Err(code) => return Err(LauncherError::AppStart(code)),
    };
    waiter.childs.len() - 1
  };

  // 判断应用进程是否立即退出
  if started_time != 0 {
    match waiter.wait_timeout((started_time as u32) * 1000) {
      Ok((child, index)) if index == app_index => {
        return Err(LauncherError::AppExited(
          started_time,
          child.exit_code().unwrap_or(u32::MAX),
        ));
      }
      Ok((child, _)) => {
        return Err(LauncherError::DependExited(
          child.name.clone(),
          child.exit_code().unwrap_or(u32::MAX),
        ));
      }
      Err(WAIT_TIMEOUT) => {}
      Err(ERROR_OPERATION_ABORTED) => return Err(LauncherError::Stopped),
      Err(code) => return Err(LauncherError::Win32(code)),
    }
  }

  match waiter.wait() {
    Ok((child, index)) if index == app_index => Ok(child.exit_code().unwrap_or(u32::MAX)),
    Ok((child, _)) => Err(LauncherError::DependExited(
      child.name.clone(),
      child.exit_code().unwrap_or(u32::MAX),
    )),
    Err(ERROR_OPERATION_ABORTED) => Err(LauncherError::Stopped),
    Err(code) => Err(LauncherError::Win32(code)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 把字符数组转成 UTF-16 码元，便于逐字符描述期望结果
  fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect::<Vec<_>>()
  }

  /// 去掉结尾的 NUL，转成普通字符串用于断言
  fn u16_to_string(wide: &[u16]) -> String {
    String::from_utf16(&wide[..wide.len() - 1]).unwrap()
  }

  #[test]
  fn quote_arg_empty_is_quoted() {
    // 空参数必须表示为 ""，否则会被命令行解析丢弃
    assert_eq!(quote_arg(""), w("\"\""));
  }

  #[test]
  fn quote_arg_without_special_chars_is_unchanged() {
    assert_eq!(quote_arg("abc"), w("abc"));
    assert_eq!(quote_arg("a/b"), w("a/b"));
    assert_eq!(quote_arg("a=b,c;d"), w("a=b,c;d"));
  }

  #[test]
  fn quote_arg_quotes_whitespace() {
    assert_eq!(quote_arg("a b"), w("\"a b\""));
    assert_eq!(quote_arg("a\tb"), w("\"a\tb\""));
    assert_eq!(quote_arg(" a"), w("\" a\""));
    assert_eq!(quote_arg("a "), w("\"a \""));
  }

  #[test]
  fn quote_arg_escapes_embedded_quote() {
    // 字面量引号前需要 2n+1 个反斜杠
    assert_eq!(quote_arg(r#"a"b"#), w(r#""a\"b""#));
    assert_eq!(quote_arg(r#"""#), w(r#""\"""#));
  }

  #[test]
  fn quote_arg_keeps_backslash_before_non_quote() {
    // 反斜杠只在其后紧跟引号时才是转义符
    assert_eq!(quote_arg("a\\b"), w("a\\b"));
    assert_eq!(quote_arg("a\\\\b c"), w("\"a\\\\b c\""));
  }

  #[test]
  fn quote_arg_doubles_trailing_backslash() {
    // 结尾反斜杠必须加倍，否则会转义结束引号
    assert_eq!(quote_arg("a b\\"), w("\"a b\\\\\""));
    assert_eq!(quote_arg("a\\\\"), w("a\\\\"));
  }

  #[test]
  fn quote_arg_escapes_backslash_before_quote() {
    // 输入 a\"b，输出 "a\\\"b"
    assert_eq!(quote_arg(r#"a\"b"#), w(r#""a\\\"b""#));
  }

  #[test]
  fn quote_arg_non_ascii_is_preserved() {
    assert_eq!(quote_arg("中文 名"), w("\"中文 名\""));
  }

  #[test]
  fn cmd_wide_is_null_terminated() {
    let cmd = cmd_wide("app.exe", &[]);
    assert_eq!(*cmd.last().unwrap(), 0);
    assert_eq!(u16_to_string(&cmd), "app.exe");
  }

  #[test]
  fn cmd_wide_joins_arguments_with_space() {
    let cmd = cmd_wide("app.exe", &["one".to_string(), "two".to_string()]);
    assert_eq!(u16_to_string(&cmd), "app.exe one two");
  }

  #[test]
  fn cmd_wide_quotes_each_part_independently() {
    let cmd = cmd_wide("my app.exe", &["a b".to_string(), "".to_string()]);
    assert_eq!(u16_to_string(&cmd), "\"my app.exe\" \"a b\" \"\"");
  }

  #[test]
  fn launcher_error_code_maps_zero_to_one() {
    // 0 表示成功，不能作为进程退出码使用
    assert_eq!(LauncherError::Win32(0).code(), 1);
    assert_eq!(LauncherError::AppStart(0).code(), 1);
    assert_eq!(LauncherError::AppExited(1, 0).code(), 1);
    assert_eq!(LauncherError::DependStart("x".to_string(), 0).code(), 1);
    assert_eq!(LauncherError::DependExited("x".to_string(), 0).code(), 1);
  }

  #[test]
  fn launcher_error_code_passes_through_win32_code() {
    assert_eq!(LauncherError::Win32(5).code(), 5);
    assert_eq!(LauncherError::AppStart(2).code(), 2);
    assert_eq!(LauncherError::AppExited(1, 3).code(), 3);
    assert_eq!(
      LauncherError::DependStart("dwarfs".to_string(), 7).code(),
      7
    );
    assert_eq!(
      LauncherError::DependExited("dwarfs".to_string(), 9).code(),
      9
    );
  }

  #[test]
  fn launcher_error_code_fixed_values() {
    assert_eq!(LauncherError::Stopped.code(), 2);
    assert_eq!(
      LauncherError::DependTimeout("dwarfs".to_string(), PathBuf::from("x")).code(),
      3
    );
  }

  #[test]
  fn waiter_wait_timeout_returns_timeout_without_children() {
    let waiter = Waiter::new().unwrap();
    // 没有子进程时不会收到任何完成通知，只会超时
    let err = waiter.wait_timeout(50).err().expect("没有子进程时应超时");
    assert_eq!(err, WAIT_TIMEOUT);
  }

  #[test]
  fn waiter_spawn_rejects_missing_program() {
    let mut waiter = Waiter::new().unwrap();
    let err = waiter
      .spawn(
        "missing",
        &Program {
          path: "win-appimg-not-a-real-program.exe".to_string(),
          args: Vec::new(),
          workdir: None,
        },
        false,
      )
      .unwrap_err();
    assert_ne!(err, 0);
    assert!(waiter.childs.is_empty());
  }

  #[test]
  fn waiter_spawn_and_wait_reports_exit_code() {
    let mut waiter = Waiter::new().unwrap();
    waiter
      .spawn(
        "cmd",
        &Program {
          path: "cmd".to_string(),
          args: ["/c".to_string(), "exit".to_string(), "3".to_string()].to_vec(),
          workdir: None,
        },
        false,
      )
      .unwrap();
    assert_eq!(waiter.childs.len(), 1);

    let mut code = None;
    for _ in 0..100 {
      match waiter.wait_timeout(1000) {
        Ok((child, index)) => {
          code = child.exit_code().map(|i| (i, index));
          break;
        }
        Err(WAIT_TIMEOUT) => {}
        Err(err) => panic!("等待子进程失败 {err}"),
      }
    }
    assert_eq!(code, Some((3, 0)));
  }

  #[test]
  fn launcher_error_display() {
    assert_eq!(LauncherError::Win32(5).to_string(), m!(Win32Error, 5));
    assert_eq!(LauncherError::AppStart(5).to_string(), m!(AppStartError, 5));
    assert_eq!(LauncherError::AppExited(3, 0).to_string(), m!(AppExited, 3));
    assert_eq!(
      LauncherError::AppExited(3, 2).to_string(),
      format!("{}{}", m!(AppExited, 3), m!(ExitCodeSuffix, 2))
    );
    assert_eq!(
      LauncherError::DependStart("DwarFS".to_string(), 5).to_string(),
      m!(DependStartError, "DwarFS", 5)
    );
    assert_eq!(
      LauncherError::DependExited("DwarFS".to_string(), 0).to_string(),
      m!(DependExited, "DwarFS")
    );
    assert_eq!(
      LauncherError::DependExited("DwarFS".to_string(), 9).to_string(),
      format!("{}{}", m!(DependExited, "DwarFS"), m!(ExitCodeSuffix, 9))
    );
    assert_eq!(
      LauncherError::DependTimeout("DwarFS".to_string(), PathBuf::from("X")).to_string(),
      m!(DependStartupTimeout, "DwarFS", "X")
    );
    assert_eq!(LauncherError::Stopped.to_string(), m!(LauncherStopped));
  }
}
