use std::{
  cmp::min,
  collections::HashMap,
  fs,
  io::{Error, ErrorKind, Result, copy},
  path::{Component, Path, PathBuf},
};

use crate::include_lang;
use dwarfs::{
  Archive, ArchiveIndex, AsChunks, Inode, InodeKind,
  archive::IsInode,
  positioned_io::{ReadAt, Size},
};

include_lang!();

/// 把文件中的一个字节区间包装成独立的随机访问文件
///
/// 用于从追加了负载的可执行文件中读取内嵌的 DwarFS 镜像。
pub struct SegmentFile {
  /// 底层文件
  #[cfg(windows)]
  file: dwarfs::positioned_io::RandomAccessFile,
  #[cfg(not(windows))]
  file: fs::File,
  /// 区间起始偏移
  offset: u64,
  /// 区间长度
  length: u64,
}

impl SegmentFile {
  /// 打开 `path`，并把它从 `offset` 开始、长度为 `length` 的区间作为独立文件
  ///
  /// `offset` 超过文件大小，或 `length` 超出 `[offset, 文件大小)` 时返回 `ErrorKind::InvalidInput`。
  pub fn open<P: AsRef<Path>>(path: P, offset: u64, length: u64) -> Result<Self> {
    #[cfg(windows)]
    let (file, size) = {
      let file = dwarfs::positioned_io::RandomAccessFile::open(path)?;
      let size = file.size()?.unwrap_or(0);
      (file, size)
    };
    #[cfg(not(windows))]
    let (file, size) = {
      let file = fs::File::open(path)?;
      let size = file.metadata()?.len();
      (file, size)
    };
    // 先比较 offset，否则 `size - offset` 在 offset > size 时会下溢
    if offset > size {
      return Err(Error::new(
        ErrorKind::InvalidInput,
        m!(OffsetOutOfRange, offset, size),
      ));
    }
    if length > size - offset {
      return Err(Error::new(
        ErrorKind::InvalidInput,
        m!(LengthOutOfRange, length, size, offset),
      ));
    }
    Ok(Self {
      file,
      offset,
      length,
    })
  }
}

/// 区间长度即该“文件”的大小，与底层文件的长度无关
impl Size for SegmentFile {
  fn size(&self) -> Result<Option<u64>> {
    Ok(Some(self.length))
  }
}

/// 以区间起点为 0 做随机读取，读取范围被限制在 `[0, length)` 之内
impl ReadAt for SegmentFile {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
    let remain = self.length.saturating_sub(offset);
    if remain == 0 || buf.is_empty() {
      return Ok(0);
    }
    let start = self.offset.saturating_add(offset);
    let max = min(buf.len() as u64, remain) as usize;
    self.file.read_at(start, &mut buf[..max])
  }
}

/// DwarFS 镜像读取器，可重复提取镜像内的不同路径
pub struct Reader {
  /// 镜像的路径索引，用于按路径查找条目
  index: ArchiveIndex,
  /// 镜像数据
  archive: Archive<SegmentFile>,
}

impl Reader {
  /// 打开位于 `path` 中 `[offset, offset + length)` 区间的 DwarFS 镜像
  ///
  /// 数据不是合法的 DwarFS 镜像时返回错误。
  pub fn new(path: &Path, offset: u64, length: u64) -> Result<Self> {
    let file = SegmentFile::open(path, offset, length)?;
    let (index, archive) = Archive::new(file)?;
    Ok(Self { index, archive })
  }

  /// 把镜像内的 `source` 条目提取到文件系统中的 `target`
  ///
  /// `source` 为空路径时表示镜像根目录。目录会被递归提取，
  /// 相同的文件 inode 会还原为硬链接。
  ///
  /// `source` 不存在时返回 `ErrorKind::NotFound`；`target` 的父目录不存在时会自动创建。
  /// `source` 为目录时 `target` 必须尚不存在，否则返回 `ErrorKind::AlreadyExists`。
  pub fn extract(&mut self, source: &Path, target: &Path) -> Result<()> {
    let mut path: Vec<String> = Vec::new();
    for i in source.components() {
      match i {
        Component::Normal(s) => path.push(s.to_string_lossy().to_string()),
        Component::ParentDir => {
          path.pop();
        }
        _ => {}
      }
    }
    let inode = if path.is_empty() {
      self.index.root().to_inode()
    } else {
      self
        .index
        .get_path(&path)
        .ok_or_else(|| Error::new(ErrorKind::NotFound, m!(PathNotFound, path.join("/"))))?
    };

    if let Some(parent) = target.parent()
      && !parent.as_os_str().is_empty()
    {
      fs::create_dir_all(parent)?;
    }

    extract_inode(&mut self.archive, inode, target, &mut HashMap::new())
  }
}

/// 提取单个 inode 到 `path`（迭代实现，避免深层目录导致栈溢出）
///
/// `inodes` 记录已写入的文件 inode 号到路径的映射，
/// 重复的 inode 会被创建为硬链接而不是复制一份数据。
///
/// 目录、符号链接、普通文件之外的条目会被静默跳过。
pub fn extract_inode(
  archive: &mut Archive<SegmentFile>,
  inode: Inode,
  path: &Path,
  inodes: &mut HashMap<u32, PathBuf>,
) -> Result<()> {
  // 显式栈：(待提取的 inode, 目标路径)
  let mut stack: Vec<(Inode, PathBuf)> = vec![(inode, path.to_path_buf())];

  while let Some((current_inode, current_path)) = stack.pop() {
    match current_inode.classify() {
      InodeKind::Directory(index) => {
        fs::create_dir(&current_path)?;
        // 子项压栈，代替递归
        for i in index.entries() {
          stack.push((i.inode(), current_path.join(i.name())));
        }
      }
      InodeKind::Symlink(link) => {
        create_symlink(link.target(), &current_path)?;
      }
      InodeKind::File(file) => {
        let inode_num = file.inode_num();
        if let Some(first) = inodes.get(&inode_num) {
          fs::hard_link(first, &current_path)?;
        } else {
          let mut data = file.as_reader(archive);
          let mut file = fs::File::create(&current_path)?;
          copy(&mut data, &mut file)?;
          inodes.insert(inode_num, current_path);
        }
      }
      _ => {}
    }
  }
  Ok(())
}

/// 一次性把 `source` 中 `[offset, offset + length)` 区间的镜像完整解压到 `target`
///
/// 需要按路径多次提取时，应复用 [`Reader`] 以避免重复解析镜像。
pub fn extract(source: &Path, offset: u64, length: u64, target: &Path) -> Result<()> {
  let mut reader = Reader::new(source, offset, length)?;
  reader.extract(Path::new(""), target)?;
  Ok(())
}

/// 创建指向 `target` 的符号链接 `link`
///
/// Unix 上不区分目标类型，直接创建符号链接。
#[cfg(unix)]
fn create_symlink(target: &str, link: &Path) -> Result<()> {
  std::os::unix::fs::symlink(target, link)
}

/// 创建指向 `target` 的符号链接 `link`
///
/// Windows 上按目标类型分别创建文件链接或目录链接：
/// 目录链接创建失败（通常缺少开发者模式或权限）时退化为 junction，
/// 文件链接创建失败但目标存在时退化为复制文件，以便无权限环境仍能提取。
#[cfg(windows)]
fn create_symlink(target: &str, link: &Path) -> Result<()> {
  let target_path = link.parent().unwrap_or_else(|| Path::new(".")).join(target);
  let (exists, is_dir) = match target_path.metadata() {
    Ok(m) => (true, m.is_dir()),
    Err(_) => (false, false),
  };

  match if is_dir {
    std::os::windows::fs::symlink_dir(target, link)
  } else {
    std::os::windows::fs::symlink_file(target, link)
  } {
    Err(_) if is_dir => junction::create(&target_path, link),
    Err(_) if exists => {
      fs::copy(target_path, link)?;
      Ok(())
    }
    r => r,
  }
}
/// 其他平台不支持符号链接，静默跳过该条目
#[cfg(not(any(unix, windows)))]
fn create_symlink(_: &str, _: &Path) -> Result<()> {
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;
  use tempfile::{NamedTempFile, TempDir};

  use crate::dwarfs_enc::{CompressParam, Packer};

  /// 创建一个测试用的文件树
  fn source_tree() -> TempDir {
    let root = tempdir();
    write_file(&root.path().join("root.txt"), b"root content");
    write_file(&root.path().join("dir").join("hello.txt"), b"hello");
    write_file(
      &root.path().join("dir").join("nested").join("deep.txt"),
      b"deep content",
    );
    root
  }

  fn tempdir() -> TempDir {
    tempfile::tempdir().unwrap()
  }

  fn write_file(path: &Path, data: &[u8]) {
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    f.write_all(data).unwrap();
  }

  /// 把目录打包成 DwarFS 镜像，返回镜像文件与其长度
  fn pack(dir: &Path) -> (NamedTempFile, u64) {
    let file = NamedTempFile::new().unwrap();
    Packer::from(dir, file.reopen().unwrap(), CompressParam::Zstd(1))
      .unwrap()
      .finish()
      .unwrap();
    let length = file.as_file().metadata().unwrap().len();
    (file, length)
  }

  fn raw_file(data: &[u8]) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(data).unwrap();
    file.flush().unwrap();
    file
  }

  fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
  }

  #[test]
  fn segment_file_open_rejects_length_over_size() {
    let file = raw_file(b"0123456789");
    let err = SegmentFile::open(file.path(), 0, 11).err().unwrap();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert_eq!(err.to_string(), m!(LengthOutOfRange, 11, 10, 0));
  }

  #[test]
  fn segment_file_open_rejects_missing_file() {
    let dir = tempdir();
    assert!(SegmentFile::open(dir.path().join("missing.bin"), 0, 0).is_err());
  }

  #[test]
  fn segment_file_open_accepts_exact_size() {
    let file = raw_file(b"0123456789");
    assert!(SegmentFile::open(file.path(), 0, 10).is_ok());
    assert!(SegmentFile::open(file.path(), 4, 10).is_err());
  }

  #[test]
  fn segment_file_open_rejects_offset_beyond_size() {
    let file = raw_file(b"0123456789");
    // offset > size 时 `size - offset` 会下溢，必须先比较 offset
    let err = SegmentFile::open(file.path(), 11, 0).err().unwrap();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert_eq!(err.to_string(), m!(OffsetOutOfRange, 11, 10));
  }

  #[test]
  fn segment_file_open_accepts_empty_segment_at_eof() {
    let file = raw_file(b"0123456789");
    // offset == 文件大小 且 length == 0 是合法边界
    let segment = SegmentFile::open(file.path(), 10, 0).unwrap();
    assert_eq!(segment.size().unwrap(), Some(0));
    let mut buf = [0u8; 4];
    assert_eq!(segment.read_at(0, &mut buf).unwrap(), 0);
  }

  #[test]
  fn segment_file_open_rejects_length_crossing_eof() {
    let file = raw_file(b"0123456789");
    // 区间为 [8, 12)，越过文件末尾
    assert!(SegmentFile::open(file.path(), 8, 4).is_err());
    assert!(SegmentFile::open(file.path(), 8, 2).is_ok());
  }

  #[test]
  fn segment_file_size_is_configured_length() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 3, 4).unwrap();
    assert_eq!(segment.size().unwrap(), Some(4));
  }

  #[test]
  fn segment_file_read_at_is_offset_by_segment_start() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 3, 4).unwrap();

    let mut buf = [0u8; 4];
    assert_eq!(segment.read_at(0, &mut buf).unwrap(), 4);
    assert_eq!(&buf, b"3456");

    let mut buf = [0u8; 2];
    assert_eq!(segment.read_at(2, &mut buf).unwrap(), 2);
    assert_eq!(&buf, b"56");
  }

  #[test]
  fn segment_file_read_at_does_not_touch_buffer_beyond_segment() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 3, 4).unwrap();
    // 区间为 [3, 7)，缓冲比区间长时，超出部分不应被写入区间之后的数据
    let mut buf = [b'.'; 8];
    assert_eq!(segment.read_at(0, &mut buf).unwrap(), 4);
    assert_eq!(&buf, b"3456....");
  }

  #[test]
  fn segment_file_read_at_clamps_to_segment_length() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 3, 4).unwrap();
    // 区间为 [3, 7)，缓冲再大也只能读到区间末尾
    let mut buf = [0u8; 10];
    assert_eq!(segment.read_at(2, &mut buf).unwrap(), 2);
    assert_eq!(&buf[..2], b"56");
  }

  #[test]
  fn segment_file_read_at_beyond_length_is_empty() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 3, 4).unwrap();
    let mut buf = [0u8; 4];
    assert_eq!(segment.read_at(4, &mut buf).unwrap(), 0);
    assert_eq!(segment.read_at(100, &mut buf).unwrap(), 0);
  }

  #[test]
  fn segment_file_read_at_empty_buffer() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 0, 10).unwrap();
    assert_eq!(segment.read_at(0, &mut []).unwrap(), 0);
  }

  #[test]
  fn segment_file_read_at_huge_offset_does_not_overflow() {
    let file = raw_file(b"0123456789");
    let segment = SegmentFile::open(file.path(), 3, 4).unwrap();
    let mut buf = [0u8; 4];
    // 极大的 offset 不应让 `self.offset + offset` 回绕到区间之前
    assert_eq!(segment.read_at(u64::MAX, &mut buf).unwrap(), 0);
    assert_eq!(segment.read_at(u64::MAX - 1, &mut buf).unwrap(), 0);
  }

  #[test]
  fn reader_new_rejects_invalid_archive() {
    let file = raw_file(b"this is definitely not a dwarfs image");
    let length = file.as_file().metadata().unwrap().len();
    assert!(Reader::new(file.path(), 0, length).is_err());
  }

  #[test]
  fn extract_rejects_invalid_archive() {
    let file = raw_file(b"this is definitely not a dwarfs image");
    let length = file.as_file().metadata().unwrap().len();
    let out = tempdir();
    assert!(extract(file.path(), 0, length, out.path()).is_err());
  }

  #[test]
  fn extract_whole_archive() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();
    let target = out.path().join("app");

    extract(image.path(), 0, length, &target).unwrap();

    assert_eq!(read_to_string(&target.join("root.txt")), "root content");
    assert_eq!(
      read_to_string(&target.join("dir").join("hello.txt")),
      "hello"
    );
    assert_eq!(
      read_to_string(&target.join("dir").join("nested").join("deep.txt")),
      "deep content"
    );
    assert!(target.is_dir());
  }

  #[test]
  fn extract_rejects_existing_target_directory() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();
    // 目录条目用 create_dir 创建，目标已存在时直接失败，避免与已有内容混合
    let err = extract(image.path(), 0, length, out.path()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AlreadyExists);
  }

  #[test]
  fn extract_restores_hard_links() {
    let root = tempdir();
    write_file(&root.path().join("a.txt"), b"shared content");
    fs::hard_link(root.path().join("a.txt"), root.path().join("b.txt")).unwrap();

    let (image, length) = pack(root.path());
    let out = tempdir();
    let target = out.path().join("app");
    extract(image.path(), 0, length, &target).unwrap();

    assert_eq!(read_to_string(&target.join("a.txt")), "shared content");
    assert_eq!(read_to_string(&target.join("b.txt")), "shared content");
    // 同一 inode 的多个条目应还原为硬链接，而不是各写一份数据
    assert_eq!(
      file_id::get_file_id(target.join("a.txt")).unwrap(),
      file_id::get_file_id(target.join("b.txt")).unwrap()
    );
  }

  #[test]
  fn extract_creates_missing_parent_directories() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    extract(image.path(), 0, length, &out.path().join("a/b/c")).unwrap();
    assert_eq!(
      read_to_string(&out.path().join("a/b/c/root.txt")),
      "root content"
    );
  }

  #[test]
  fn reader_extract_subdirectory() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    let mut reader = Reader::new(image.path(), 0, length).unwrap();
    reader
      .extract(Path::new("dir"), &out.path().join("dir"))
      .unwrap();

    assert_eq!(read_to_string(&out.path().join("dir/hello.txt")), "hello");
    assert_eq!(
      read_to_string(&out.path().join("dir/nested/deep.txt")),
      "deep content"
    );
    // 子目录之外的条目不应被提取
    assert!(!out.path().join("dir/root.txt").exists());
  }

  #[test]
  fn reader_can_be_reused_for_multiple_extracts() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    // 文档建议按路径多次提取时复用 Reader，因此首次提取后镜像必须仍可用
    let mut reader = Reader::new(image.path(), 0, length).unwrap();
    reader
      .extract(Path::new("dir"), &out.path().join("first"))
      .unwrap();
    reader
      .extract(Path::new("root.txt"), &out.path().join("second.txt"))
      .unwrap();
    reader
      .extract(Path::new("dir/hello.txt"), &out.path().join("third.txt"))
      .unwrap();

    assert_eq!(read_to_string(&out.path().join("first/hello.txt")), "hello");
    assert_eq!(
      read_to_string(&out.path().join("second.txt")),
      "root content"
    );
    assert_eq!(read_to_string(&out.path().join("third.txt")), "hello");
  }

  #[test]
  fn reader_extract_single_file() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    let mut reader = Reader::new(image.path(), 0, length).unwrap();
    reader
      .extract(Path::new("root.txt"), &out.path().join("out.txt"))
      .unwrap();

    assert_eq!(read_to_string(&out.path().join("out.txt")), "root content");
  }

  #[test]
  fn reader_extract_missing_path() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    let mut reader = Reader::new(image.path(), 0, length).unwrap();
    let err = reader
      .extract(Path::new("nope"), &out.path().join("nope"))
      .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
    assert_eq!(err.to_string(), m!(PathNotFound, "nope"));
  }

  #[test]
  fn reader_extract_strips_leading_separators() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    let mut reader = Reader::new(image.path(), 0, length).unwrap();
    // 前导分隔符与 `..` 不应影响结果
    reader
      .extract(Path::new("/../root.txt"), &out.path().join("out.txt"))
      .unwrap();
    assert_eq!(read_to_string(&out.path().join("out.txt")), "root content");
  }

  #[test]
  fn reader_extract_ignores_parent_dir_escape() {
    let source = source_tree();
    let (image, length) = pack(source.path());
    let out = tempdir();

    let mut reader = Reader::new(image.path(), 0, length).unwrap();
    // 路径为空时 `..` 会被忽略，退化为根目录
    reader
      .extract(Path::new(".."), &out.path().join("all"))
      .unwrap();
    assert_eq!(
      read_to_string(&out.path().join("all/root.txt")),
      "root content"
    );
  }

  #[cfg(any(unix, windows))]
  #[test]
  fn create_symlink_creates_link() {
    let dir = tempdir();
    write_file(&dir.path().join("target.txt"), b"data");
    let link = dir.path().join("link.txt");
    create_symlink("target.txt", &link).unwrap();
    assert_eq!(read_to_string(&link), "data");
  }
}
