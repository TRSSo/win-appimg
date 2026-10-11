use std::{
  collections::HashMap,
  env, error,
  ffi::OsStr,
  fs,
  io::{Error, ErrorKind},
  path::{Path, PathBuf},
};

use regex::Regex;
use yaml_rust2::{Yaml, YamlLoader};

/// 递归收集 `path` 下所有 `.yml` 文件的路径
///
/// 结果追加到 `res`，顺序取决于文件系统枚举顺序。
/// 目录不存在或任何一级读取失败都会中断并返回错误，让构建立即失败。
fn scan_yml_files(path: &Path, res: &mut Vec<PathBuf>) -> std::io::Result<()> {
  for entry in fs::read_dir(path)? {
    let path = entry?.path();
    if path.is_dir() {
      scan_yml_files(&path, res)?
    } else if path.is_file() && path.extension() == Some(OsStr::new("yml")) {
      res.push(path)
    }
  }
  Ok(())
}

/// 把 `lang` 下的所有 `.yml` 翻译成 Rust 代码，输出到 `OUT_DIR/i18n`
///
/// 每个 `.yml` 对应一个与源文件同名的 `.rs`（`lang/lib.yml` -> `i18n/src/lib.rs`），
/// 供 [`crate::include_lang`] 宏 `include!` 进来。
///
/// 每个键的值必须是长度为 2 的数组（英文、简体中文）：
/// - 不含 `{}` 的生成 `I18nMsg` 枚举 + `I18N_MSG` 静态表，`m!` 直接查表返回；
/// - 含 `{}` 的生成 `__i18n_fmt!` 宏分支，`m!` 走 `format!`。
///
/// 是否参与格式化只看英文串，因此两种语言的占位符必须保持一致。
fn build_i18n(source_path: &Path) -> Result<(), Box<dyn error::Error>> {
  println!("cargo:rerun-if-changed={}", source_path.display());
  let output_path = PathBuf::from(env::var("OUT_DIR").unwrap());
  let format = Regex::new(r"\{[^{}]*\}")?;

  let mut paths = Vec::new();
  scan_yml_files(source_path, &mut paths)?;
  let mut msg_map: HashMap<String, (&Path, (String, String))> = HashMap::new();
  for path in &paths {
    let yaml = YamlLoader::load_from_str(&fs::read_to_string(path)?)?;
    let rows = if let Some(Yaml::Hash(rows)) = yaml.first() {
      rows
    } else {
      return Err(
        Error::new(
          ErrorKind::InvalidData,
          format!("无法解析为对象 {}", path.display()),
        )
        .into(),
      );
    };

    let mut msg = Vec::new();
    let mut msgf = Vec::new();

    for (key, value) in rows {
      let key_str = key.as_str().ok_or_else(|| {
        Error::new(
          ErrorKind::InvalidData,
          format!("YAML键无法解析为字符串 {}", path.display()),
        )
      })?;

      if let Yaml::Array(arr) = value {
        if arr.len() == 2 {
          let str0 = arr[0].as_str().ok_or_else(|| {
            Error::new(
              ErrorKind::InvalidData,
              format!("YAML值0无法解析为字符串 {}", path.display()),
            )
          })?;
          let str1 = arr[1].as_str().ok_or_else(|| {
            Error::new(
              ErrorKind::InvalidData,
              format!("YAML值1无法解析为字符串 {}", path.display()),
            )
          })?;

          let format_len0 = format.find_iter(str0).count();
          let format_len1 = format.find_iter(str1).count();
          if format_len0 != format_len1 {
            return Err(
              Error::new(
                ErrorKind::InvalidData,
                format!(
                  "格式数量不一致 {} {str0}({format_len0}) {str1}({format_len1})",
                  path.display()
                ),
              )
              .into(),
            );
          } else if format_len0 != 0 {
            msgf.push((key_str, str0, str1))
          } else {
            msg.push((key_str, str0, str1))
          }
          let k = key_str.to_string();
          let v = (path.as_path(), (str0.to_string(), str1.to_string()));
          if let Some(value) = msg_map.get(&k) {
            if value.1 != v.1 {
              return Err(
                Error::new(
                  ErrorKind::InvalidData,
                  format!(
                    "{k} 翻译不一致 {}: {:?} {}: {:?}",
                    v.0.display(),
                    v.1,
                    value.0.display(),
                    value.1
                  ),
                )
                .into(),
              );
            }
          } else {
            msg_map.insert(k, v);
          }
        } else {
          return Err(
            Error::new(
              ErrorKind::InvalidData,
              format!("YAML数组长度必须为2 {}", path.display()),
            )
            .into(),
          );
        }
      } else {
        return Err(
          Error::new(
            ErrorKind::InvalidData,
            format!("YAML值无法解析为数组 {}", path.display()),
          )
          .into(),
        );
      }
    }

    // bin 目标是独立 crate，只能通过包名引用；库内部则用 crate 自身
    let is_bin = path.starts_with(source_path.join("bin"));
    let mut code = if path == &source_path.join("lib.yml") {
      // lib.rs 内 m!、Lang 已在作用域中，再 use 会与自身定义冲突
      String::new()
    } else {
      let pkg_name = if is_bin {
        env!("CARGO_PKG_NAME").replace('-', "_")
      } else {
        "crate".to_string()
      };
      let mut items = Vec::new();
      // bin 目标的入口需要 init_lang 初始化语言环境
      if is_bin {
        items.push("init_lang");
      }
      // Lang 只被 __i18n_fmt! 用到，没有格式化文案时引入会触发 unused_imports
      if !msgf.is_empty() {
        items.push("Lang");
      }
      items.push("m");
      format!(
        "use {pkg_name}::{{{}}};\n{}",
        items.join(", "),
        if is_bin { "\n" } else { "" }
      )
    };

    if !msg.is_empty() {
      code.push_str("enum I18nMsg {\n");
      for i in &msg {
        code.push_str(&format!("  {},\n", i.0));
      }
      code.push_str("}\n");

      code.push_str(&format!("const I18N_MSG: [[&str; 2]; {}] = [\n", msg.len()));
      for i in &msg {
        code.push_str(&format!("  [{:?}, {:?}],\n", i.1, i.2));
      }
      code.push_str("];\n");
    }

    if !msgf.is_empty() {
      code.push_str("macro_rules! __i18n_fmt {\n");
      for i in &msgf {
        code.push_str(&format!(
          "  ({}, $lang:expr $(, $arg:expr)* $(,)?) => {{\n",
          i.0
        ));
        code.push_str("    match $lang {\n");
        code.push_str(&format!(
          "      Lang::En => format!({:?} $(, $arg)*),\n",
          i.1
        ));
        code.push_str(&format!(
          "      Lang::ZhCn => format!({:?} $(, $arg)*),\n",
          i.2
        ));
        code.push_str("    }\n");
        code.push_str("  };\n");
      }
      code.push_str("}\n");
    }

    let mut output_path = output_path
      .join("i18n/src")
      .join(path.strip_prefix(source_path)?);
    output_path.set_file_name(format!(
      "{}.rs",
      output_path
        .file_stem()
        .ok_or_else(|| {
          Error::new(
            ErrorKind::InvalidData,
            format!("路径无法解析 {}", output_path.display()),
          )
        })?
        .display()
    ));
    if let Some(parent) = output_path.parent() {
      fs::create_dir_all(parent)?;
    }
    fs::write(output_path, code)?
  }
  Ok(())
}

/// 构建入口：先生成国际化代码，再在 Windows 上编译资源（图标、版本信息、清单）
///
/// 资源相关内容放在 `res` 目录，不存在时跳过。
fn main() -> Result<(), Box<dyn error::Error>> {
  build_i18n(Path::new("lang"))?;

  #[cfg(all(windows, any(feature = "launcher", feature = "post_build")))]
  {
    // build_i18n 已声明过 rerun-if-changed，Cargo 不再默认监视整个包，
    // 因此资源目录必须显式声明，否则改动图标或清单后不会重新编译资源
    println!("cargo:rerun-if-changed=res");
    let path = Path::new("res");

    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", env!("CARGO_PKG_DESCRIPTION"));
    res.set(
      "LegalCopyright",
      &format!(
        "© {} {} | {}",
        time::OffsetDateTime::now_utc().year(),
        Regex::new(r"\s+<.+?>")?.replace_all(env!("CARGO_PKG_AUTHORS"), ""),
        env!("CARGO_PKG_LICENSE")
      ),
    );

    let tempfile = tempfile::NamedTempFile::new()?;
    svg_to_ico::svg_to_ico(
      &path.join("logo.svg"),
      96.0,
      tempfile.path(),
      &[16, 24, 32, 48, 64, 128, 256],
    )
    .map_err(|e| format!("转换图标失败: {e}"))?;
    res.set_icon(&tempfile.path().to_string_lossy());

    let manifest = fs::read_to_string(path.join("manifest.xml"))?
      .replace("0.0.0.0", concat!(env!("CARGO_PKG_VERSION"), ".0"));
    let manifest = Regex::new(r"(>?)\s+(<|/?>)")?.replace_all(&manifest, "$1$2");
    let manifest = Regex::new(r"\s+")?.replace_all(&manifest, " ");
    res.set_manifest(&manifest);

    res.compile()?
  }
  Ok(())
}
