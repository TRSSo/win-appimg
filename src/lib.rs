pub mod dwarfs;
pub mod dwarfs_enc;
pub mod payload;
#[cfg(windows)]
pub mod win;

use std::{
  io::{Error, ErrorKind, Result},
  path::{Path, PathBuf},
};

use path_clean::PathClean;

include_lang!();

/// 支持的界面语言
///
/// 枚举值与 `I18N_MSG` 的第二维下标一一对应，改动时需要保持同步。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
  /// 英语
  En = 0,
  /// 简体中文
  ZhCn = 1,
}

/// 当前使用的语言，由 [`init_lang`] 在启动时设置，之后只读
///
/// 这是全局可变静态量，只能在启动阶段写入；
/// 若将来需要在运行时切换语言，应改为原子类型或 [`std::sync::OnceLock`]。
pub static mut LANG: Lang = Lang::En;

/// 初始化语言环境
///
/// 只能在程序启动时调用一次
pub fn init_lang() {
  #[cfg(debug_assertions)]
  if let Ok(lang) = std::env::var("DEBUG_LANG") {
    if lang.starts_with("zh") {
      unsafe { LANG = Lang::ZhCn };
    }
    return;
  }

  if let Some(lang) = sys_locale::get_locale()
    && lang.starts_with("zh")
  {
    unsafe { LANG = Lang::ZhCn }
  }
}

/// 按当前语言取出翻译文案
///
/// - `m!(Key)`：不含占位符的文案，直接查 `I18N_MSG` 表返回 `&'static str`
/// - `m!(Key, 参数...)`：含 `{}` 占位符的文案，走 `format!` 返回 `String`
///
/// 两种形式的返回类型不同，拼接时注意用 `&` 或 `to_string()` 统一。
/// 使用前所在模块必须先调用 [`include_lang!`]，且键名需存在于同名的 `lang/*.yml` 中，
/// 否则会在编译期报错。
#[macro_export]
macro_rules! m {
  ($msg:ident) => {
    I18N_MSG[I18nMsg::$msg as usize][unsafe { $crate::LANG as usize }]
  };
  ($msg:ident, $($arg:expr),+ $(,)?) => {
    __i18n_fmt!($msg, unsafe { $crate::LANG }, $($arg),+)
  };
}

/// 引入当前源文件对应的国际化代码
///
/// 由 `build.rs` 依据 `lang/` 下与源文件同名的 `.yml` 生成，
/// 展开后提供 `I18nMsg`、`I18N_MSG` 与 `__i18n_fmt!`，供 [`m!`] 使用；
/// `lang/bin/*.yml` 还会额外引入 `init_lang`。
///
/// 每个使用 [`m!`] 的源文件都需要在顶层调用一次。
#[macro_export]
macro_rules! include_lang {
  () => {
    include!(concat!(env!("OUT_DIR"), "/i18n/", file!()));
  };
}

/// 路径转换扩展，提供绝对路径与相对路径之间的互相转换
///
/// 所有转换结果都会经过 [`path_clean::PathClean::clean`] 规范化。
// `from_absolute` 语义上取的是“基准目录”而非 Rust 约定的 `From` 转换。
#[allow(clippy::wrong_self_convention)]
pub trait PathConvert {
  /// 以 `from` 为基准目录把路径转为绝对路径
  ///
  /// `self` 为绝对路径时原样返回，否则拼接到 `from` 之后。
  fn from_absolute(&self, from: impl AsRef<Path>) -> PathBuf;

  /// 把路径转为绝对路径
  ///
  /// `self` 为绝对路径时原样返回，否则拼接到当前工作目录之后。
  fn to_absolute(&self) -> Result<PathBuf>;

  /// 计算从 `from` 到 `self` 的相对路径
  ///
  /// `self` 不在 `from` 之内时允许通过 `..` 向上跳出。
  fn to_relative(&self, from: impl AsRef<Path>) -> Result<PathBuf>;

  /// 计算从 `from` 到 `self` 的相对路径，并限制结果不能跳出 `from`
  ///
  /// `self` 不在 `from` 之内时返回 `ErrorKind::InvalidInput`。
  fn to_relative_limit(&self, from: impl AsRef<Path>) -> Result<PathBuf>;
}

impl PathConvert for Path {
  fn from_absolute(&self, from: impl AsRef<Path>) -> PathBuf {
    from_absolute(self, from)
  }
  fn to_absolute(&self) -> Result<PathBuf> {
    to_absolute(self)
  }
  fn to_relative(&self, from: impl AsRef<Path>) -> Result<PathBuf> {
    to_relative(from, self, false)
  }
  fn to_relative_limit(&self, from: impl AsRef<Path>) -> Result<PathBuf> {
    to_relative(from, self, true)
  }
}

impl PathConvert for PathBuf {
  fn from_absolute(&self, from: impl AsRef<Path>) -> PathBuf {
    from_absolute(self, from)
  }
  fn to_absolute(&self) -> Result<PathBuf> {
    to_absolute(self)
  }
  fn to_relative(&self, from: impl AsRef<Path>) -> Result<PathBuf> {
    to_relative(from, self, false)
  }
  fn to_relative_limit(&self, from: impl AsRef<Path>) -> Result<PathBuf> {
    to_relative(from, self, true)
  }
}

/// 以 `from` 为基准目录把 `path` 规范化为绝对路径
fn from_absolute(path: impl AsRef<Path>, from: impl AsRef<Path>) -> PathBuf {
  let path = path.as_ref();
  if path.is_absolute() {
    path.to_path_buf()
  } else {
    from.as_ref().join(path)
  }
  .clean()
}

/// 把 `path` 规范化为绝对路径，相对路径相对当前工作目录解析
///
/// 读取当前工作目录失败时返回错误。
fn to_absolute(path: impl AsRef<Path>) -> Result<PathBuf> {
  let path = path.as_ref();
  Ok(
    if path.is_absolute() {
      path.to_path_buf()
    } else {
      std::env::current_dir()?.join(path)
    }
    .clean(),
  )
}

/// 计算从 `from` 到 `to` 的相对路径，两端都会先解析为绝对路径
///
/// 逐组件比较公共前缀；`limit` 为 `true` 时 `to` 必须位于 `from` 之内，
/// 否则返回 `ErrorKind::InvalidInput`；为 `false` 时允许用 `..` 向上跳出。
/// 两端完全相同时返回 `.`。
fn to_relative(from: impl AsRef<Path>, to: impl AsRef<Path>, limit: bool) -> Result<PathBuf> {
  let from = from.as_ref().to_absolute()?;
  let from: Vec<_> = from.components().collect();
  let to = to.as_ref().to_absolute()?;
  let to: Vec<_> = to.components().collect();
  let same = from
    .iter()
    .zip(to.iter())
    .take_while(|(a, b)| a == b)
    .count();
  if limit && same < from.len() {
    return Err(Error::new(ErrorKind::InvalidInput, m!(PathOutOfLimit)));
  }
  let mut path = PathBuf::new();
  for _ in same..from.len() {
    path.push("..");
  }
  for i in &to[same..] {
    path.push(i);
  }
  if path.as_os_str().is_empty() {
    path.push(".");
  }
  Ok(path)
}

/// 生成长度为 `length` 的随机字母数字字符串
///
/// 用作临时目录名、挂载点名等，只保证尽量不重复，不适用于安全相关场景。
pub fn tempname(length: usize) -> String {
  std::iter::repeat_with(fastrand::alphanumeric)
    .take(length)
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 当前工作目录，测试中作为绝对基准路径使用
  fn base() -> PathBuf {
    std::env::current_dir().unwrap().clean()
  }

  #[test]
  fn from_absolute_absolute_path_is_cleaned() {
    let base = base();
    let path = base.join("a").join("..").join("b");
    assert_eq!(path.from_absolute(&base), base.join("b"));
    assert_eq!(base.from_absolute(&base), base);
  }

  #[test]
  fn from_absolute_relative_path_is_joined() {
    let base = base();
    assert_eq!(Path::new("a").from_absolute(&base), base.join("a"));
    assert_eq!(
      Path::new("a/b/c").from_absolute(&base),
      base.join("a").join("b").join("c")
    );
    assert_eq!(Path::new(".").from_absolute(&base), base);
    assert_eq!(
      Path::new("..").from_absolute(&base),
      base.parent().unwrap().to_path_buf()
    );
  }

  #[test]
  fn from_absolute_ignores_base_when_absolute() {
    let base = base();
    let other = base.join("other");
    assert_eq!(base.from_absolute(&other), base);
  }

  #[test]
  fn to_absolute_keeps_absolute_path() {
    let base = base();
    assert_eq!(base.to_absolute().unwrap(), base);
  }

  #[test]
  fn to_absolute_joins_current_dir() {
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(Path::new("a").to_absolute().unwrap(), cwd.join("a").clean());
    assert_eq!(
      Path::new("a/../b").to_absolute().unwrap(),
      cwd.join("b").clean()
    );
  }

  #[test]
  fn to_relative_same_path_is_current_dir() {
    let base = base();
    assert_eq!(base.to_relative(&base).unwrap(), Path::new("."));
    // 相对路径会被解析到当前工作目录
    assert_eq!(Path::new(".").to_relative(&base).unwrap(), Path::new("."));
  }

  #[test]
  fn to_relative_child_path() {
    let base = base();
    assert_eq!(base.join("a").to_relative(&base).unwrap(), Path::new("a"));
    assert_eq!(
      base.join("a").join("b").to_relative(&base).unwrap(),
      Path::new("a").join("b")
    );
  }

  #[test]
  fn to_relative_parent_path() {
    let base = base();
    assert_eq!(
      base.parent().unwrap().to_relative(&base).unwrap(),
      Path::new("..")
    );
  }

  #[test]
  fn to_relative_sibling_path() {
    let base = base().join("a");
    assert_eq!(
      base.parent().unwrap().join("b").to_relative(&base).unwrap(),
      Path::new("..").join("b")
    );
  }

  #[test]
  fn to_relative_accepts_relative_input() {
    let base = base();
    assert_eq!(
      Path::new("a/b").to_relative(&base).unwrap(),
      Path::new("a").join("b")
    );
  }

  #[test]
  fn to_relative_limit_allows_path_inside_base() {
    let base = base();
    assert_eq!(base.to_relative_limit(&base).unwrap(), Path::new("."));
    assert_eq!(
      base.join("a").to_relative_limit(&base).unwrap(),
      Path::new("a")
    );
    assert_eq!(
      base.join("a/b/c").to_relative_limit(&base).unwrap(),
      Path::new("a").join("b").join("c")
    );
  }

  #[test]
  fn to_relative_limit_rejects_path_outside_base() {
    let base = base().join("a");
    let err = base.parent().unwrap().to_relative_limit(&base).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains(m!(PathOutOfLimit)));
  }

  #[test]
  fn to_relative_distinguishes_prefix_siblings() {
    let base = base();
    // "abc" 与 "abd" 只是字符串前缀相同，不应被当作包含关系
    assert_eq!(
      base.join("abd").to_relative(base.join("abc")).unwrap(),
      Path::new("..").join("abd")
    );
  }

  #[test]
  fn to_relative_limit_rejects_prefix_sibling() {
    let base = base();
    let err = base
      .join("abd")
      .to_relative_limit(base.join("abc"))
      .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
  }

  #[test]
  fn to_relative_limit_allows_without_limit() {
    let base = base().join("a");
    // 与 to_relative_limit 相同的输入，未限制时应当成功
    assert_eq!(
      base.parent().unwrap().to_relative(&base).unwrap(),
      Path::new("..")
    );
  }

  #[test]
  fn path_convert_is_implemented_for_pathbuf() {
    let base = base();
    let buf: PathBuf = base.join("a");
    assert_eq!(buf.to_relative(&base).unwrap(), Path::new("a"));
    assert_eq!(buf.to_relative_limit(&base).unwrap(), Path::new("a"));
    assert_eq!(buf.to_absolute().unwrap(), buf);
    assert_eq!(Path::new("a").from_absolute(&base), buf);
  }

  #[test]
  fn lang_matches_i18n_msg_layout() {
    // Lang 的枚举值即 I18N_MSG 的第二维下标，改动其一必须同步
    assert_eq!(Lang::En as usize, 0);
    assert_eq!(Lang::ZhCn as usize, 1);

    let msg = I18N_MSG[I18nMsg::PathOutOfLimit as usize];
    assert_eq!(msg.len(), 2);
    assert!(!msg[Lang::En as usize].is_empty());
    assert!(!msg[Lang::ZhCn as usize].is_empty());
    assert_ne!(msg[Lang::En as usize], msg[Lang::ZhCn as usize]);
    // 未调用 init_lang 时 LANG 为 Lang::En，m! 应取英文
    assert_eq!(m!(PathOutOfLimit), msg[Lang::En as usize]);
  }

  #[test]
  fn tempname_length() {
    assert_eq!(tempname(0), "");
    assert_eq!(tempname(1).len(), 1);
    assert_eq!(tempname(6).len(), 6);
    assert_eq!(tempname(64).len(), 64);
  }

  #[test]
  fn tempname_is_alphanumeric() {
    assert!(tempname(128).chars().all(|c| c.is_ascii_alphanumeric()));
  }

  #[test]
  fn tempname_is_not_constant() {
    assert_ne!(tempname(32), tempname(32));
  }
}
