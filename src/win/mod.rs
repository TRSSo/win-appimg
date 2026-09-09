use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

/// 依赖管理：检查、安装与卸载 WinFsp 及其组件
pub mod depend;
/// 图形界面对话框（TaskDialog 封装）
pub mod dialog;
/// 子进程启动、Job 分组与等待
pub mod launch;
/// 应用镜像启动器
pub mod launcher;
/// 以管理员权限运行程序
pub mod sudo;

/// 字符串转换扩展，提供到 Windows 宽字符串的转换
pub trait StringConvert {
  /// 转换为以 NUL 结尾的 UTF-16 码元序列
  ///
  /// 内容中已有的 NUL 会被过滤掉，否则 Windows API 会在该处提前截断字符串；
  /// 非 BMP 字符会被编码为代理对。空字符串返回只含结尾 NUL 的 `[0]`。
  fn to_wide(&self) -> Vec<u16>;
}

impl StringConvert for str {
  fn to_wide(&self) -> Vec<u16> {
    self
      .encode_utf16()
      .filter(|c| *c != 0)
      .chain(std::iter::once(0))
      .collect()
  }
}

impl StringConvert for OsStr {
  fn to_wide(&self) -> Vec<u16> {
    self
      .encode_wide()
      .filter(|c| *c != 0)
      .chain(std::iter::once(0))
      .collect()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 去掉结尾的 NUL 后还原为 `String`，用于校验转换结果
  fn from_wide(wide: &[u16]) -> String {
    assert_eq!(wide.last().copied(), Some(0), "结果必须以 NUL 结尾");
    String::from_utf16(&wide[..wide.len() - 1]).unwrap()
  }

  #[test]
  fn to_wide_appends_nul_terminator() {
    assert_eq!("ab".to_wide(), vec![b'a' as u16, b'b' as u16, 0]);
  }

  #[test]
  fn to_wide_empty_string_is_only_nul() {
    assert_eq!("".to_wide(), vec![0]);
  }

  #[test]
  fn to_wide_roundtrips_text() {
    for text in ["app.exe", "C:\\路径\\file.txt", "a b", "-o"] {
      assert_eq!(from_wide(&text.to_wide()), text, "text = {text:?}");
    }
  }

  #[test]
  fn to_wide_encodes_non_bmp_as_surrogate_pair() {
    // U+1D11E 在 UTF-16 中需要两个码元
    let wide = "\u{1D11E}".to_wide();
    assert_eq!(wide, vec![0xD834, 0xDD1E, 0]);
    assert_eq!(from_wide(&wide), "\u{1D11E}");
  }

  #[test]
  fn to_wide_filters_embedded_nul() {
    // 嵌入的 NUL 会被过滤，否则 Windows API 会在此处截断字符串
    assert_eq!("a\0b".to_wide(), vec![b'a' as u16, b'b' as u16, 0]);
    assert_eq!(from_wide(&"a\0b".to_wide()), "ab");
  }

  #[test]
  fn os_str_to_wide_matches_str() {
    let text = "C:\\Program Files\\WinFsp\\bin";
    assert_eq!(OsStr::new(text).to_wide(), text.to_wide());
    assert_eq!(from_wide(&OsStr::new(text).to_wide()), text);
  }

  #[test]
  fn os_str_to_wide_empty_is_only_nul() {
    assert_eq!(OsStr::new("").to_wide(), vec![0]);
  }
}
