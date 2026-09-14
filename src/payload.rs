use std::{
  cmp::min,
  fs::File,
  io::{Error, ErrorKind, Read, Result, Seek, SeekFrom},
  path::Path,
};

use crate::{dwarfs, include_lang};
use editpe::Image;
use xxhash_rust::xxh3::Xxh3Default;
use zerompk::{FromMessagePack, ToMessagePack, from_msgpack};

include_lang!();

/// 一段内嵌数据的位置与校验信息
#[derive(Debug, ToMessagePack, FromMessagePack)]
pub struct PayloadData {
  /// 数据在文件中的起始偏移
  pub offset: u64,
  /// 数据长度
  pub length: u64,
  /// 数据内容的 xxh3 摘要
  pub hash: u64,
}

/// 构建期写入的信息，描述 `cli` 自身携带的依赖包与启动程序
#[derive(Debug, ToMessagePack, FromMessagePack)]
pub struct Payload {
  /// 依赖包（WinFsp + DwarFS + OverlayFS）
  pub depend: PayloadData,
  /// 启动程序 `launcher.exe`
  pub launcher: PayloadData,
}

/// 可写层的存储方式
#[derive(Debug, ToMessagePack, FromMessagePack, PartialEq)]
pub enum WritableType {
  /// 只读，不使用可写层
  None,
  /// 保存在程序同目录的 `.data` 文件夹
  Save,
  /// 保存在临时文件夹，程序退出后丢弃
  Temp,
  /// 保存在内存文件系统
  Memory,
}

/// 运行期读取的信息，描述一个已打包应用镜像的启动方式
#[derive(Debug, ToMessagePack, FromMessagePack)]
pub struct LauncherPayload {
  /// 依赖包
  pub depend: PayloadData,
  /// 应用镜像
  pub image: PayloadData,

  /// 启动程序路径，相对路径相对镜像挂载点解析
  pub exe_path: String,
  /// 启动程序固定参数，运行时传入的参数会追加在其后
  ///
  /// 其中的 `{}` 会在启动时替换为应用根目录，详见 [`crate::format_args`]；
  /// 运行时追加的参数不做替换。
  pub exe_args: Vec<String>,
  /// 工作目录，相对路径相对镜像挂载点解析
  pub exe_workdir: Option<String>,

  /// 为 `true` 时启动镜像外部的原始路径，不使用镜像内的程序
  pub exe_external: bool,
  /// 启动后在该秒数内退出则视为启动失败，为 0 时不检查
  pub exe_started_time: u16,
  /// 为 `true` 时显示控制台窗口
  pub exe_console: bool,

  /// 可写层的存储方式
  pub exe_writable: WritableType,
}

/// 与 `.payload` 段内占位符对应的 `offset` + `length` 结构
///
/// 构建时由 `post_build` 把占位符替换为真实的信息偏移与长度。
#[repr(C)]
struct PayloadOffset {
  /// 信息在文件中的起始偏移
  offset: u64,
  /// 信息长度
  length: u64,
}

/// `.payload` 段中的占位符，长度必须正好为 16 字节（两个 `u64`）
///
/// 中间的分隔符是私有区字符 `U+F05F`，正常情况下不会出现在可执行文件里，
/// 因此可以按字节搜索定位占位符而不易误命中。
pub const PAYLOAD_PLACEHOLDER: &[u8] = "PAYLOAD\u{F05F}OFFSET".as_bytes();

/// 放在自身 `.payload` 段里的占位数据，构建后期会被替换为真实的信息偏移与长度
///
/// 初值与 [`PAYLOAD_PLACEHOLDER`] 的字节完全一致，
/// 因此 `post_build` 可以按字节搜索定位到这两个 `u64` 并回填。
#[cfg_attr(target_os = "windows", unsafe(link_section = ".payload"))]
static PAYLOAD_OFFSET: PayloadOffset = {
  let mut offset = [0u8; 8];
  let mut length = [0u8; 8];
  let mut i = 0;
  while i < 8 {
    offset[i] = PAYLOAD_PLACEHOLDER[i];
    i += 1;
  }
  while i < 16 {
    length[i - 8] = PAYLOAD_PLACEHOLDER[i];
    i += 1;
  }
  PayloadOffset {
    offset: u64::from_le_bytes(offset),
    length: u64::from_le_bytes(length),
  }
};

/// 从自身可执行文件的 `.payload` 段读取信息偏移，并反序列化为 `T`
///
/// 信息偏移超过文件长度或信息不是合法的 MessagePack 数据时返回错误。
pub fn get_payload<T: for<'a> FromMessagePack<'a>>(mut file: &std::fs::File) -> Result<T> {
  let file_length = file.metadata()?.len();
  let payload_offset = unsafe { std::ptr::read_volatile(&PAYLOAD_OFFSET) };

  if file_length < payload_offset.offset
    || payload_offset.length > file_length - payload_offset.offset
  {
    return Err(Error::new(
      ErrorKind::InvalidData,
      m!(
        InvalidPayloadLength,
        file_length,
        payload_offset.offset,
        payload_offset.length
      ),
    ));
  }

  file.seek(SeekFrom::Start(payload_offset.offset))?;
  let mut data = vec![0u8; payload_offset.length as usize];
  file.read_exact(&mut data)?;

  let payload = from_msgpack(&data)
    .map_err(|e| Error::new(ErrorKind::InvalidData, m!(PayloadDeserializeError, e)))?;

  Ok(payload)
}

/// 在 PE 镜像中查找 `.payload` 段的文件偏移
///
/// 不存在该段时返回 `ErrorKind::NotFound`。
pub fn find_pe_offset(image: &Image) -> Result<u64> {
  for i in image.section_table().iter() {
    if i.name().as_deref() == Some(".payload") {
      return Ok(i.pointer_to_raw_data as u64);
    }
  }
  Err(Error::new(ErrorKind::NotFound, m!(PayloadOffsetNotFound)))
}

/// 在文件中查找 `PAYLOAD_PLACEHOLDER` 的文件偏移
///
/// 不存在该段时返回 `ErrorKind::NotFound`。
pub fn find_offset(file: &mut File) -> Result<u64> {
  let mut data = Vec::new();
  file.read_to_end(&mut data)?;
  data
    .windows(PAYLOAD_PLACEHOLDER.len())
    .position(|x| x == PAYLOAD_PLACEHOLDER)
    .map_or(
      Err(Error::new(ErrorKind::NotFound, m!(PayloadOffsetNotFound))),
      |pos| Ok(pos as u64),
    )
}

/// 默认读写缓冲大小。
///
/// 可根据场景调整：
/// - NVMe / 本地 SSD: 256 KiB ~ 1 MiB
/// - HDD: 1 MiB 左右通常不错
/// - 网络存储 / 对象存储下载流: 可适当增大
pub const DEFAULT_BUF_SIZE: usize = 1024 * 1024;

/// 从 `reader` 读取 `length` 字节并计算 xxh3 摘要，返回摘要值
///
/// 读取过程中提前遇到 EOF 时返回 `ErrorKind::UnexpectedEof`；
/// `length` 为 0 时不读取任何数据，返回空输入的摘要。
pub fn hash_read<R: Read>(mut reader: R, length: u64) -> Result<u64> {
  let mut hasher = Xxh3Default::new();
  // 缓冲区不超过待读取长度，避免为小数据也分配整个默认缓冲
  let mut buf = vec![0u8; min(DEFAULT_BUF_SIZE as u64, length) as usize];
  let mut remaining = length;

  while remaining > 0 {
    let max = min(buf.len() as u64, remaining) as usize;
    let n = reader.read(&mut buf[..max])?;

    if n == 0 {
      return Err(Error::new(
        ErrorKind::UnexpectedEof,
        m!(HashError, show_length(length), show_length(remaining)),
      ));
    }

    hasher.update(&buf[..n]);
    remaining -= n as u64;
  }

  Ok(hasher.digest())
}

/// 计算整个文件的 xxh3 摘要，会先把文件指针回绕到开头
pub fn hash_file(file: &mut File) -> Result<u64> {
  let length = file.metadata()?.len();
  file.seek(SeekFrom::Start(0))?;
  hash_read(file, length)
}

/// 校验 `[offset, offset + length)` 落在文件大小之内
///
/// `offset` 越界返回 `ErrorKind::InvalidInput`，`offset + length` 越界返回 `ErrorKind::UnexpectedEof`。
fn validate_file_range(file: &File, offset: u64, length: u64) -> Result<()> {
  let file_len = file.metadata()?.len();
  if offset > file_len {
    return Err(Error::new(
      ErrorKind::InvalidInput,
      m!(OffsetBeyondFile, offset, file_len),
    ));
  }
  if length > file_len - offset {
    return Err(Error::new(
      ErrorKind::UnexpectedEof,
      m!(RangeBeyondFile, offset, length, file_len),
    ));
  }
  Ok(())
}

/// 把字节数格式化为人类可读的字符串，如 `1.50 MiB`
///
/// 使用 1024 进制，不足 1 KiB 时以 `B` 为单位且不带小数。
pub fn show_length(length: u64) -> String {
  const UNITS: &[&str] = &["B", "K", "M", "G", "T", "P", "E"];
  let mut length_f = length as f64;
  let mut i = 0;
  while length_f >= 1024.0 && i < UNITS.len() - 1 {
    length_f /= 1024.0;
    i += 1;
  }
  if i == 0 {
    format!("{} {}", length, UNITS[i])
  } else {
    format!("{:.2} {}iB", length_f, UNITS[i])
  }
}

impl PayloadData {
  /// 校验 `file` 中本段数据的摘要是否与 `self.hash` 一致
  ///
  /// 返回 `Ok(false)` 表示数据不匹配，`Err` 表示无法读取该区间。
  pub fn hash(&self, file: &mut File) -> Result<bool> {
    validate_file_range(file, self.offset, self.length)?;
    file.seek(SeekFrom::Start(self.offset))?;
    Ok(self.hash == hash_read(file, self.length)?)
  }

  /// 先校验摘要，再把 `file` 中本段的 DwarFS 镜像解压到 `output_path`
  ///
  /// `path` 为 `file` 对应的文件路径，仅用于传递给解压器与拼装错误信息。
  /// 摘要不匹配时返回 `ErrorKind::InvalidData` 且不会解压。
  pub fn extract(&self, path: &Path, file: &mut File, output_path: &Path) -> Result<()> {
    if !self.hash(file)? {
      return Err(Error::new(ErrorKind::InvalidData, m!(DataVerifyFailed)));
    }
    dwarfs::extract(path, self.offset, self.length, output_path)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;
  use tempfile::NamedTempFile;
  use zerompk::to_msgpack_vec;

  /// 直接计算 xxh3 摘要，作为 hash_read 的期望值
  fn xxh3(data: &[u8]) -> u64 {
    let mut hasher = Xxh3Default::new();
    hasher.update(data);
    hasher.digest()
  }

  /// 每次最多返回 `chunk` 字节的读取器，用于验证 hash_read 的分块循环
  struct Chunked<'a> {
    data: &'a [u8],
    pos: usize,
    chunk: usize,
  }
  impl Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
      if self.pos >= self.data.len() {
        return Ok(0);
      }
      let n = min(self.chunk, min(buf.len(), self.data.len() - self.pos));
      buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
      self.pos += n;
      Ok(n)
    }
  }

  fn temp_with(data: &[u8]) -> NamedTempFile {
    use std::io::Write;
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(data).unwrap();
    file.flush().unwrap();
    file
  }

  #[test]
  fn payload_placeholder_is_16_bytes() {
    // 占位符会被链接器替换为 offset(u64) + length(u64)，因此必须正好 16 字节
    assert_eq!(PAYLOAD_PLACEHOLDER.len(), 16);
  }

  #[test]
  fn get_payload_rejects_unfilled_placeholder() {
    // 测试程序未经 post_build 回填，`.payload` 段仍是占位符，
    // 其字节被解析为一个远超文件长度的偏移，必须报错而不是读到垃圾数据
    let file = temp_with(b"win-appimg");
    let err = get_payload::<PayloadData>(&file.reopen().unwrap()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
  }

  #[test]
  fn show_length_bytes() {
    assert_eq!(show_length(0), "0 B");
    assert_eq!(show_length(1), "1 B");
    assert_eq!(show_length(512), "512 B");
    assert_eq!(show_length(1023), "1023 B");
    // 不足 1 KiB 时不使用小数
    assert!(!show_length(1023).contains('.'));
  }

  #[test]
  fn show_length_scales_units() {
    assert_eq!(show_length(1024), "1.00 KiB");
    assert_eq!(show_length(1024 * 1024), "1.00 MiB");
    assert_eq!(show_length(1024u64.pow(3)), "1.00 GiB");
    assert_eq!(show_length(1024u64.pow(4)), "1.00 TiB");
    assert_eq!(show_length(1024u64.pow(5)), "1.00 PiB");
    assert_eq!(show_length(1024u64.pow(6)), "1.00 EiB");
    assert_eq!(show_length(u64::MAX), "16.00 EiB");
  }

  #[test]
  fn show_length_fractional_values() {
    assert_eq!(show_length(1024 + 512), "1.50 KiB");
    assert_eq!(show_length(1024 * 1024 * 3 / 2), "1.50 MiB");
    // 边界：刚好小于下一级单位
    assert_eq!(show_length(1024 * 1024 - 1), "1024.00 KiB");
  }

  #[test]
  fn show_length_unit_boundaries() {
    // 恰好比下一级单位小 1 时仍使用当前单位
    assert_eq!(show_length(1024u64.pow(2) - 1), "1024.00 KiB");
    assert_eq!(show_length(1024u64.pow(3) - 1), "1024.00 MiB");
    assert_eq!(show_length(1024u64.pow(4) - 1), "1024.00 GiB");
  }

  #[test]
  fn show_length_is_monotonic() {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut last: Option<(usize, f64)> = None;
    for i in 0..64 {
      let text = show_length(1u64 << i);
      // 不应产生空串或 NaN
      assert!(!text.is_empty(), "1 << {i}");
      assert!(!text.contains("NaN"), "1 << {i} = {text}");

      let (value, unit) = text.split_once(' ').expect("数值与单位之间应有空格");
      let value: f64 = value.parse().expect("数值部分应可解析");
      let unit = UNITS.iter().position(|u| *u == unit).expect("单位应合法");
      // 输入递增时，单位不应降级；同一单位内数值必须递增
      if let Some((last_unit, last_value)) = last {
        assert!(unit >= last_unit, "1 << {i} = {text}");
        if unit == last_unit {
          assert!(value > last_value, "1 << {i} = {text}");
        }
      }
      last = Some((unit, value));
    }
  }

  #[test]
  fn hash_read_empty_length() {
    assert_eq!(hash_read(Cursor::new(Vec::new()), 0).unwrap(), xxh3(&[]));
    // 长度 0 时不读取任何数据，多余的输入被忽略
    assert_eq!(hash_read(Cursor::new(b"abc"), 0).unwrap(), xxh3(&[]));
  }

  #[test]
  fn hash_read_matches_xxh3() {
    let data = b"hello win-appimg";
    assert_eq!(
      hash_read(Cursor::new(data), data.len() as u64).unwrap(),
      xxh3(data)
    );
  }

  #[test]
  fn hash_read_only_reads_requested_length() {
    let data = b"abcdef";
    assert_eq!(hash_read(Cursor::new(data), 3).unwrap(), xxh3(b"abc"));
  }

  #[test]
  fn hash_read_is_independent_of_chunk_size() {
    // 数据大于默认缓冲，且每次只返回少量字节，用于覆盖多次循环
    let data: Vec<u8> = (0..(DEFAULT_BUF_SIZE as u64 * 2 + 1234))
      .map(|i| (i % 251) as u8)
      .collect();
    let expected = xxh3(&data);
    for chunk in [1, 7, 4096, DEFAULT_BUF_SIZE, data.len()] {
      let reader = Chunked {
        data: &data,
        pos: 0,
        chunk,
      };
      assert_eq!(
        hash_read(reader, data.len() as u64).unwrap(),
        expected,
        "chunk = {chunk}"
      );
    }
  }

  #[test]
  fn hash_read_different_data_differs() {
    assert_ne!(
      hash_read(Cursor::new(b"a"), 1).unwrap(),
      hash_read(Cursor::new(b"b"), 1).unwrap()
    );
    assert_ne!(
      hash_read(Cursor::new(b"ab"), 2).unwrap(),
      hash_read(Cursor::new(b"ba"), 2).unwrap()
    );
  }

  #[test]
  fn hash_read_unexpected_eof() {
    let err = hash_read(Cursor::new(b"abc"), 4).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    // 读到 3 字节后遇到 EOF，仍缺 1 字节
    assert_eq!(
      err.to_string(),
      m!(HashError, show_length(4), show_length(1))
    );
  }

  #[test]
  fn hash_read_immediate_eof() {
    let err = hash_read(Cursor::new(Vec::new()), 1).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
  }

  #[test]
  fn hash_file_matches_content() {
    let file = temp_with(b"win-appimg payload");
    let mut file = file.reopen().unwrap();
    assert_eq!(hash_file(&mut file).unwrap(), xxh3(b"win-appimg payload"));
  }

  #[test]
  fn hash_file_empty_file() {
    let file = temp_with(b"");
    let mut file = file.reopen().unwrap();
    assert_eq!(hash_file(&mut file).unwrap(), xxh3(&[]));
  }

  #[test]
  fn hash_file_rewinds_before_hashing() {
    let file = temp_with(b"0123456789");
    let mut file = file.reopen().unwrap();
    // 先读取一部分，hash_file 应当先回到文件开头
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).unwrap();
    assert_eq!(hash_file(&mut file).unwrap(), xxh3(b"0123456789"));
  }

  #[test]
  fn payload_data_hash_matches() {
    let content = b"0123456789";
    let file = temp_with(content);
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 2,
      length: 5,
      hash: xxh3(b"23456"),
    };
    assert!(data.hash(&mut file).unwrap());
  }

  #[test]
  fn payload_data_hash_is_repeatable() {
    let file = temp_with(b"0123456789");
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 2,
      length: 5,
      hash: xxh3(b"23456"),
    };
    // 每次校验都会重新定位到区间起点，连续调用结果应一致
    assert!(data.hash(&mut file).unwrap());
    assert!(data.hash(&mut file).unwrap());
  }

  #[test]
  fn payload_data_hash_mismatch() {
    let file = temp_with(b"0123456789");
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 0,
      length: 10,
      hash: 0,
    };
    assert!(!data.hash(&mut file).unwrap());
  }

  #[test]
  fn payload_data_hash_zero_length_at_eof() {
    let file = temp_with(b"abc");
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 3,
      length: 0,
      hash: xxh3(&[]),
    };
    // offset == 文件长度 且 length == 0 是合法边界
    assert!(data.hash(&mut file).unwrap());
  }

  #[test]
  fn payload_data_hash_rejects_offset_beyond_eof() {
    let file = temp_with(b"abc");
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 4,
      length: 0,
      hash: 0,
    };
    let err = data.hash(&mut file).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert_eq!(err.to_string(), m!(OffsetBeyondFile, 4, 3));
  }

  #[test]
  fn payload_data_hash_rejects_range_beyond_eof() {
    let file = temp_with(b"abc");
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 2,
      length: 2,
      hash: 0,
    };
    let err = data.hash(&mut file).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
  }

  #[test]
  fn payload_data_extract_reads_image_at_offset() {
    use crate::dwarfs_enc::{CompressParam, Packer};
    use std::io::Write;

    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("hello.txt"), b"payload extract").unwrap();
    let image = NamedTempFile::new().unwrap();
    Packer::from(
      source.path(),
      image.reopen().unwrap(),
      CompressParam::Zstd(1),
    )
    .unwrap()
    .finish()
    .unwrap();
    let image_bytes = std::fs::read(image.path()).unwrap();

    // 模拟真实产物布局：镜像位于启动器之后的非零偏移处
    const PREFIX: &[u8] = b"launcher.exe placeholder";
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(PREFIX).unwrap();
    file.write_all(&image_bytes).unwrap();
    file.flush().unwrap();

    let data = PayloadData {
      offset: PREFIX.len() as u64,
      length: image_bytes.len() as u64,
      hash: xxh3(&image_bytes),
    };
    let mut opened = file.reopen().unwrap();
    assert!(data.hash(&mut opened).unwrap());

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("app");
    data.extract(file.path(), &mut opened, &target).unwrap();
    assert_eq!(
      std::fs::read_to_string(target.join("hello.txt")).unwrap(),
      "payload extract"
    );
  }

  #[test]
  fn payload_data_extract_rejects_mismatched_hash() {
    let file = temp_with(b"not a dwarfs image");
    let mut file = file.reopen().unwrap();
    let data = PayloadData {
      offset: 0,
      length: 18,
      hash: 0,
    };
    let err = data
      .extract(Path::new(""), &mut file, Path::new("out"))
      .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(err.to_string(), m!(DataVerifyFailed));
  }

  #[test]
  fn payload_data_msgpack_roundtrip() {
    let data = PayloadData {
      offset: u64::MAX,
      length: 1234,
      hash: u64::MAX - 1,
    };
    let bytes = to_msgpack_vec(&data).unwrap();
    let decoded: PayloadData = from_msgpack(&bytes).unwrap();
    assert_eq!(decoded.offset, data.offset);
    assert_eq!(decoded.length, data.length);
    assert_eq!(decoded.hash, data.hash);
  }

  #[test]
  fn payload_msgpack_roundtrip() {
    let payload = Payload {
      depend: PayloadData {
        offset: 1,
        length: 2,
        hash: 3,
      },
      launcher: PayloadData {
        offset: 4,
        length: 5,
        hash: 6,
      },
    };
    let bytes = to_msgpack_vec(&payload).unwrap();
    let decoded: Payload = from_msgpack(&bytes).unwrap();
    assert_eq!(decoded.depend.offset, 1);
    assert_eq!(decoded.launcher.hash, 6);
  }

  #[test]
  fn launcher_payload_msgpack_roundtrip() {
    let payload = LauncherPayload {
      depend: PayloadData {
        offset: 1,
        length: 2,
        hash: 3,
      },
      image: PayloadData {
        offset: 4,
        length: 5,
        hash: 6,
      },
      exe_path: "app.exe".to_string(),
      exe_args: vec!["--flag".to_string(), "a b".to_string()],
      exe_workdir: Some("work".to_string()),
      exe_external: false,
      exe_started_time: 2,
      exe_console: true,
      exe_writable: WritableType::Memory,
    };
    let bytes = to_msgpack_vec(&payload).unwrap();
    let decoded: LauncherPayload = from_msgpack(&bytes).unwrap();
    assert_eq!(decoded.exe_path, "app.exe");
    assert_eq!(
      decoded.exe_args,
      vec!["--flag".to_string(), "a b".to_string()]
    );
    assert_eq!(decoded.exe_workdir.as_deref(), Some("work"));
    assert!(!decoded.exe_external);
    assert!(decoded.exe_console);
    assert_eq!(decoded.exe_started_time, 2);
    assert_eq!(decoded.exe_writable, WritableType::Memory);
  }

  #[test]
  fn launcher_payload_optional_workdir_roundtrip() {
    let payload = LauncherPayload {
      depend: PayloadData {
        offset: 0,
        length: 0,
        hash: 0,
      },
      image: PayloadData {
        offset: 0,
        length: 0,
        hash: 0,
      },
      exe_path: String::new(),
      exe_args: Vec::new(),
      exe_workdir: None,
      exe_external: true,
      exe_started_time: 0,
      exe_console: false,
      exe_writable: WritableType::None,
    };
    let bytes = to_msgpack_vec(&payload).unwrap();
    let decoded: LauncherPayload = from_msgpack(&bytes).unwrap();
    assert!(decoded.exe_workdir.is_none());
    assert!(decoded.exe_args.is_empty());
    assert_eq!(decoded.exe_writable, WritableType::None);
  }

  #[test]
  fn writable_type_roundtrip() {
    for i in [
      WritableType::None,
      WritableType::Save,
      WritableType::Temp,
      WritableType::Memory,
    ] {
      let bytes = to_msgpack_vec(&i).unwrap();
      assert_eq!(from_msgpack::<WritableType>(&bytes).unwrap(), i);
    }
  }

  #[test]
  fn msgpack_rejects_truncated_data() {
    let bytes = to_msgpack_vec(&PayloadData {
      offset: 1,
      length: 2,
      hash: 3,
    })
    .unwrap();
    assert!(from_msgpack::<PayloadData>(&bytes[..bytes.len() - 1]).is_err());
  }
}
