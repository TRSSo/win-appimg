use std::{
  ffi::OsStr,
  io::Result,
  path::Path,
  process::Command,
  ptr::{null, null_mut},
};

use crate::win::StringConvert;
use windows_sys::{
  Win32::{
    System::Console::GetConsoleWindow,
    UI::{
      Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_ERROR_ICON,
        TD_INFORMATION_ICON, TD_SHIELD_ICON, TD_WARNING_ICON, TDCBF_CANCEL_BUTTON,
        TDCBF_CLOSE_BUTTON, TDCBF_NO_BUTTON, TDCBF_OK_BUTTON, TDCBF_RETRY_BUTTON, TDCBF_YES_BUTTON,
        TDF_ALLOW_DIALOG_CANCELLATION, TaskDialog, TaskDialogIndirect,
      },
      WindowsAndMessaging::{IDCANCEL, IDCLOSE, IDNO, IDOK, IDRETRY, IDYES, SW_HIDE, ShowWindow},
    },
  },
  core::PCWSTR,
};

/// TaskDialog 图标
pub enum Icon {
  /// 无图标
  None,
  /// 信息（蓝色 i）
  Info,
  /// 警告（黄色感叹号）
  Warning,
  /// 盾牌（UAC 提权提示）
  Shield,
  /// 错误（红色叉号）
  Error,
}

/// 把图标转换为 TaskDialog 使用的图标指针，[`Icon::None`] 对应空指针
impl From<Icon> for PCWSTR {
  fn from(value: Icon) -> PCWSTR {
    match value {
      Icon::None => null(),
      Icon::Info => TD_INFORMATION_ICON,
      Icon::Warning => TD_WARNING_ICON,
      Icon::Shield => TD_SHIELD_ICON,
      Icon::Error => TD_ERROR_ICON,
    }
  }
}

/// TaskDialog 按钮
///
/// 注意两个转换方向使用的是 Windows 里两套不同的编号，并不互逆：
/// - `From<Button> for i32` 产出 `TDCBF_*` 位掩码，用于指定要显示哪些按钮；
/// - `From<i32> for Button` 解析 TaskDialog 返回的 `ID*` 按钮 ID。
///
/// 因此 `Button::from(i32::from(b))` 只有在 `Button::Ok` 上才等于 `b`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
  /// 确定
  Ok,
  /// 是
  Yes,
  /// 否
  No,
  /// 取消
  Cancel,
  /// 重试
  Retry,
  /// 关闭
  Close,
  /// 未知按钮，保存原始的按钮 ID
  Unknown(i32),
}

/// 把按钮转换为 TaskDialog 的按钮位掩码位，用于按位或后传给 [`msgbox`]
impl From<Button> for i32 {
  fn from(value: Button) -> Self {
    match value {
      Button::Ok => TDCBF_OK_BUTTON,
      Button::Yes => TDCBF_YES_BUTTON,
      Button::No => TDCBF_NO_BUTTON,
      Button::Cancel => TDCBF_CANCEL_BUTTON,
      Button::Retry => TDCBF_RETRY_BUTTON,
      Button::Close => TDCBF_CLOSE_BUTTON,
      Button::Unknown(i) => i,
    }
  }
}

/// 把 TaskDialog 返回的按钮 ID 转换回按钮，无法识别的 ID 保留为 [`Button::Unknown`]
impl From<i32> for Button {
  fn from(value: i32) -> Self {
    match value {
      IDOK => Self::Ok,
      IDYES => Self::Yes,
      IDNO => Self::No,
      IDCANCEL => Self::Cancel,
      IDRETRY => Self::Retry,
      IDCLOSE => Self::Close,
      i => Self::Unknown(i),
    }
  }
}
/// 显示一个 TaskDialog 对话框并返回用户点击的按钮
///
/// - `icon`：标题栏与主指令旁显示的图标
/// - `title`：窗口标题，会自动追加程序名与版本号
/// - `head`：主指令
/// - `content`：补充说明，`None` 时不显示
/// - `buttons`：要显示的按钮，会按位或成按钮掩码
///
/// 对话框创建失败时会 `panic`。
pub fn msgbox(
  icon: Icon,
  title: &str,
  head: &str,
  content: Option<&str>,
  buttons: &[Button],
) -> Button {
  let title_w = format!(
    "{title} - {} v{}",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_VERSION")
  )
  .to_wide();
  let head_w = head.to_wide();
  let content_w = content.map(|s| s.to_wide());
  let flags = buttons.iter().copied().fold(0, |r, i| r | i32::from(i));
  let mut res = 0;
  if let code = unsafe {
    TaskDialog(
      null_mut(),
      null_mut(),
      title_w.as_ptr(),
      head_w.as_ptr(),
      content_w.as_ref().map_or(null(), |s| s.as_ptr()),
      flags,
      icon.into(),
      &mut res,
    )
  } && code < 0
  {
    panic!("TaskDialog 调用失败 {code}")
  }
  Button::from(res)
}

/// 确定 对话框
///
/// 用法：`ok!(图标, 标题, 主指令[, 补充说明])`，只能点“确定”，返回值可忽略。
#[macro_export]
macro_rules! ok {
  ($icon:expr, $title:expr, $head:expr) => {
    dialog::msgbox($icon, $title, $head, None, &[dialog::Button::Ok])
  };
  ($icon:expr, $title:expr, $head:expr, $content:expr) => {
    dialog::msgbox($icon, $title, $head, Some($content), &[dialog::Button::Ok])
  };
}

/// 确定 取消 对话框
///
/// 用法：`okcancel!(标题, 主指令[, 补充说明])`，固定使用信息图标。
/// 点“确定”返回 `true`，取消或关闭返回 `false`。
#[macro_export]
macro_rules! okcancel {
  ($title:expr, $head:expr) => {
    match dialog::msgbox(
      dialog::Icon::Info,
      $title,
      $head,
      None,
      &[dialog::Button::Ok, dialog::Button::Cancel],
    ) {
      dialog::Button::Ok => true,
      _ => false,
    }
  };
  ($title:expr, $head:expr, $content:expr) => {
    match dialog::msgbox(
      dialog::Icon::Info,
      $title,
      $head,
      Some($content),
      &[dialog::Button::Ok, dialog::Button::Cancel],
    ) {
      dialog::Button::Ok => true,
      _ => false,
    }
  };
}

/// 是(Y) 否(N) 对话框
///
/// 用法：`yesno!(标题, 主指令[, 补充说明])`，固定使用信息图标。
/// 点“是”返回 `true`，否或关闭返回 `false`。
#[macro_export]
macro_rules! yesno {
  ($title:expr, $head:expr) => {
    match dialog::msgbox(
      dialog::Icon::Info,
      $title,
      $head,
      None,
      &[dialog::Button::Yes, dialog::Button::No],
    ) {
      dialog::Button::Yes => true,
      _ => false,
    }
  };
  ($title:expr, $head:expr, $content:expr) => {
    match dialog::msgbox(
      dialog::Icon::Info,
      $title,
      $head,
      Some($content),
      &[dialog::Button::Yes, dialog::Button::No],
    ) {
      dialog::Button::Yes => true,
      _ => false,
    }
  };
}

/// 是(Y) 否(N) 取消 对话框
///
/// 用法：`yesnocancel!(标题, 主指令[, 补充说明])`，固定使用信息图标。
/// 点“是”返回 `Some(true)`，“否”返回 `Some(false)`，取消或关闭返回 `None`。
#[macro_export]
macro_rules! yesnocancel {
  ($title:expr, $head:expr) => {
    match dialog::msgbox(
      dialog::Icon::Info,
      $title,
      $head,
      None,
      &[
        dialog::Button::Yes,
        dialog::Button::No,
        dialog::Button::Cancel,
      ],
    ) {
      dialog::Button::Yes => Some(true),
      dialog::Button::No => Some(false),
      _ => None,
    }
  };
  ($title:expr, $head:expr, $content:expr) => {
    match dialog::msgbox(
      dialog::Icon::Info,
      $title,
      $head,
      Some($content),
      &[
        dialog::Button::Yes,
        dialog::Button::No,
        dialog::Button::Cancel,
      ],
    ) {
      dialog::Button::Yes => Some(true),
      dialog::Button::No => Some(false),
      _ => None,
    }
  };
}

/// 重试(R) 取消 对话框
///
/// 用法：`retry!(标题, 主指令[, 补充说明])`，固定使用错误图标。
/// 点“重试”返回 `true`，取消或关闭返回 `false`。
#[macro_export]
macro_rules! retry {
  ($title:expr, $head:expr) => {
    match dialog::msgbox(
      dialog::Icon::Error,
      $title,
      $head,
      None,
      &[dialog::Button::Retry, dialog::Button::Cancel],
    ) {
      dialog::Button::Retry => true,
      _ => false,
    }
  };
  ($title:expr, $head:expr, $content:expr) => {
    match dialog::msgbox(
      dialog::Icon::Error,
      $title,
      $head,
      Some($content),
      &[dialog::Button::Retry, dialog::Button::Cancel],
    ) {
      dialog::Button::Retry => true,
      _ => false,
    }
  };
}

/// 确定 重试(R) 关闭(C) 对话框
///
/// 用法：`retryclose!(标题, 主指令[, 补充说明])`，固定使用警告图标。
/// 点“确定”返回 `Some(true)`，“重试”返回 `Some(false)`，关闭返回 `None`。
#[macro_export]
macro_rules! retryclose {
  ($title:expr, $head:expr) => {
    match dialog::msgbox(
      dialog::Icon::Warning,
      $title,
      $head,
      None,
      &[
        dialog::Button::Ok,
        dialog::Button::Retry,
        dialog::Button::Close,
      ],
    ) {
      dialog::Button::Ok => Some(true),
      dialog::Button::Retry => Some(false),
      _ => None,
    }
  };
  ($title:expr, $head:expr, $content:expr) => {
    match dialog::msgbox(
      dialog::Icon::Warning,
      $title,
      $head,
      Some($content),
      &[
        dialog::Button::Ok,
        dialog::Button::Retry,
        dialog::Button::Close,
      ],
    ) {
      dialog::Button::Ok => Some(true),
      dialog::Button::Retry => Some(false),
      _ => None,
    }
  };
}

/// 自定义选项框
///
/// 用法：`option!(图标, 标题, 主指令[, 补充说明], 选项数组)`，
/// 默认选中第一项且允许取消，返回值含义见 [`optionbox`]。
#[macro_export]
macro_rules! option {
  ($icon:expr, $title:expr, $head:expr, $options:expr) => {
    dialog::optionbox($icon, $title, $head, None, $options, None, true)
  };
  ($icon:expr, $title:expr, $head:expr, $content:expr, $options:expr) => {
    dialog::optionbox($icon, $title, $head, Some($content), $options, None, true)
  };
}

/// 自定义选项框，每个选项对应一个按钮
///
/// - `icon`：标题栏与主指令旁显示的图标
/// - `title`：窗口标题
/// - `head`：主指令
/// - `content`：补充说明，`None` 时不显示
/// - `options`：选项文字，按数组顺序对应按钮
/// - `default_index`：默认选中的选项下标，越界或 `None` 时选中第一个
/// - `allow_cancel`：为 `true` 时允许通过 Esc / 关闭按钮取消
///
/// 返回被点击的选项下标；`options` 为空、用户取消或对话框被关闭时返回 `None`。
/// 对话框创建失败时会 `panic`。
pub fn optionbox(
  icon: Icon,
  title: &str,
  head: &str,
  content: Option<&str>,
  options: &[&str],
  default_index: Option<usize>,
  allow_cancel: bool,
) -> Option<usize> {
  // 没有选项时无法构造 TaskDialog 按钮，直接返回 `None` 而不是越界访问
  if options.is_empty() {
    return None;
  }

  let title_w = title.to_wide();
  let head_w = head.to_wide();
  let content_w = content.map(|s| s.to_wide());

  let option_w: Vec<Vec<u16>> = options.iter().map(|s| s.to_wide()).collect();

  let buttons: Vec<TASKDIALOG_BUTTON> = option_w
    .iter()
    .enumerate()
    .map(|(i, text)| TASKDIALOG_BUTTON {
      nButtonID: 10 + i as i32,
      pszButtonText: text.as_ptr(),
    })
    .collect();

  let config = TASKDIALOGCONFIG {
    cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
    pszWindowTitle: title_w.as_ptr(),
    pszMainInstruction: head_w.as_ptr(),
    pszContent: content_w.as_ref().map_or(null(), |s| s.as_ptr()),

    cButtons: buttons.len() as u32,
    pButtons: buttons.as_ptr(),

    nDefaultButton: match default_index {
      Some(idx) if idx < options.len() => 10 + idx as i32,
      _ => buttons[0].nButtonID,
    },

    dwFlags: if allow_cancel {
      TDF_ALLOW_DIALOG_CANCELLATION
    } else {
      Default::default()
    },

    // 设置 TaskDialog 图标
    Anonymous1: TASKDIALOGCONFIG_0 {
      pszMainIcon: icon.into(),
    },

    ..Default::default()
  };

  let mut res = 0;
  if let code = unsafe { TaskDialogIndirect(&config, &mut res, null_mut(), null_mut()) }
    && code < 0
  {
    panic!("TaskDialogIndirect 调用失败 {code}")
  }

  if res >= 10 {
    let idx = (res - 10) as usize;
    if idx < options.len() {
      return Some(idx);
    }
  }
  None
}

/// 隐藏控制台窗口
///
/// 进程没有关联控制台时什么都不做。
pub fn hide_console() {
  unsafe {
    let hwnd = GetConsoleWindow();
    if !hwnd.is_null() {
      ShowWindow(hwnd, SW_HIDE);
    }
  }
}

/// 在 Windows 资源管理器中打开并选中指定文件
///
/// 只负责启动 `explorer` 进程，不等待其结束，也不检查 `path` 是否存在；
/// 无法启动 `explorer` 时返回错误。
pub fn show_path(path: &Path) -> Result<()> {
  let parent = path.parent().unwrap_or(Path::new("."));
  let name = path.file_name().unwrap_or(OsStr::new("."));
  Command::new("explorer")
    .current_dir(parent)
    .arg("/select,")
    .arg(name)
    .spawn()?;
  Ok(())
}

/// 用系统默认浏览器打开 URL
///
/// 只负责启动 `explorer` 进程，不等待其结束；无法启动 `explorer` 时返回错误。
pub fn open_url(url: &str) -> Result<()> {
  Command::new("explorer").arg(url).spawn()?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 所有按钮对应的位掩码位，用于校验转换结果
  const ALL_BUTTONS: [Button; 6] = [
    Button::Ok,
    Button::Yes,
    Button::No,
    Button::Cancel,
    Button::Retry,
    Button::Close,
  ];

  #[test]
  fn button_to_i32_bits_do_not_overlap() {
    // 每个按钮占一位，才能按位或成按钮掩码
    let mut union = 0;
    for i in ALL_BUTTONS {
      let bit = i32::from(i);
      assert!(bit.is_positive(), "{i:?} 应为单个位");
      assert_eq!(bit & (bit - 1), 0, "{i:?} 应为单个位");
      assert_eq!(union & bit, 0, "{i:?} 与其他按钮位重叠");
      union |= bit;
    }
  }

  #[test]
  fn button_from_i32_keeps_unknown_id() {
    // 3 是 IDABORT，5 是 IDIGNORE，都不在 IDOK..IDCLOSE 之内
    for id in [0, 3, 5, 100, -1] {
      assert_eq!(Button::from(id), Button::Unknown(id), "id = {id}");
    }
  }

  #[test]
  fn optionbox_returns_none_for_empty_options() {
    // 没有选项时无法构造 TaskDialog 按钮，应在调用 Windows API 之前返回
    assert_eq!(
      optionbox(Icon::Info, "title", "head", None, &[], None, true),
      None
    );
    assert_eq!(
      optionbox(
        Icon::Info,
        "title",
        "head",
        Some("content"),
        &[],
        Some(3),
        false
      ),
      None
    );
  }
}
