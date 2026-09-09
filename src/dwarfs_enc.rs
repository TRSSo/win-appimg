use std::{
  collections::HashMap,
  fs,
  io::{Error, ErrorKind, Result, Write},
  path::{Path, PathBuf},
  time::SystemTime,
};

use crate::{PathConvert, include_lang};
use dwarfs_enc::{
  chunker::{BasicChunker, CdcChunker, Chunker},
  metadata::{Builder as MetaBuilder, DirId, FileId, InodeMetadata, LinkableInodeId},
  section::Writer as SectionWriter,
};
use path_clean::PathClean;

pub use ::dwarfs_enc::section::CompressParam;

include_lang!();

/// DwarFS 镜像打包器，把文件系统中的条目写入 `W`
///
/// 指向同一个文件（同一 inode）的多个条目通过文件唯一标识识别并复用为硬链接，
/// 已创建的目录会被缓存以避免重复创建。
pub struct Packer<W: Write> {
  /// 镜像元数据构建器
  builder: MetaBuilder,
  /// 分块写入器，负责去重与压缩块数据
  chunker: CdcChunker<W>,
  /// 元数据的压缩参数
  compress: CompressParam,
  /// 已写入文件的唯一标识（卷 + 文件号）到 inode 的映射，用于识别硬链接
  hardlinks: HashMap<file_id::FileId, LinkableInodeId>,
  /// 已创建目录的**镜像内路径**到目录 ID 的映射
  ///
  /// 键一律是镜像内路径，混入文件系统路径会让 `create_parent` 误命中，
  /// 把后续条目静默放到别的目录下。
  dirs: HashMap<PathBuf, DirId>,
}

impl<W: Write> Packer<W> {
  /// 以 `path` 目录的元数据作为镜像根目录，并递归添加其全部内容
  ///
  /// `path` 必须是目录，否则返回错误。之后仍可继续调用 [`Packer::add`] 追加条目。
  pub fn from(path: &Path, output: W, compress: CompressParam) -> Result<Self> {
    let mut packer = Self::new(InodeMetadata::from(&path.metadata()?), output, compress)?;
    let mut stack = fs::read_dir(path)?
      .map(|i| {
        let name = i?.file_name();
        Ok((
          packer.builder.root(),
          name.to_string_lossy().to_string(),
          path.join(&name),
          PathBuf::from(&name),
        ))
      })
      .collect::<Result<Vec<(DirId, String, PathBuf, PathBuf)>>>()?;
    packer.add_id(path, &mut stack)?;
    Ok(packer)
  }

  /// 以 `metadata` 作为镜像根目录的元数据，创建一个空的打包器
  ///
  /// 与 [`Packer::from`] 不同，这里不会读取任何文件，条目需要通过 [`Packer::add`] 添加。
  pub fn new(metadata: InodeMetadata, output: W, compress: CompressParam) -> Result<Self> {
    let builder = MetaBuilder::new(&metadata);
    let writer = SectionWriter::new(output)?;
    let chunker = CdcChunker::new(BasicChunker::new(writer, builder.block_size(), compress));
    Ok(Self {
      builder,
      chunker,
      compress,
      hardlinks: HashMap::new(),
      dirs: HashMap::new(),
    })
  }

  /// 把文件系统中的 `source` 添加为镜像内的 `target`
  ///
  /// `source` 是目录时会递归添加其全部内容，
  /// `target` 为镜像内的路径，开头**不要**带 `/`，中间目录会自动创建。
  ///
  /// `target` 的文件名为空（如 `..`、空路径）时返回 `ErrorKind::InvalidInput`。
  pub fn add(&mut self, source: &Path, target: &Path) -> Result<()> {
    let target = target.clean();
    let parent = self.create_parent(&target)?;
    let name = target
      .file_name()
      .ok_or_else(|| Error::new(ErrorKind::InvalidInput, m!(InvalidFileName)))?;
    self.add_id(
      source,
      &mut vec![(
        parent,
        name.to_string_lossy().to_string(),
        source.to_path_buf(),
        target,
      )],
    )
  }

  /// 依次弹出 `stack` 中的条目写入镜像，目录的子项会压回栈中，代替递归
  ///
  /// `stack` 的元素为 `(父目录 ID, 条目名, 文件系统路径, 镜像内路径)`。
  /// 目录条目会以**镜像内路径**为键登记到 `dirs` 供 `create_parent` 复用，
  /// 因此第四个分量必须是镜像内路径；传入文件系统路径会让后续 [`Packer::add`]
  /// 误命中缓存，把条目静默放到别的目录下。
  ///
  /// `root` 为被打包的**文件系统**根目录，用于判断符号链接是否指向该目录内部：
  /// 指向内部的链接会被改写为相对路径，指向外部的保持原样。
  ///
  /// 符号链接与目录 junction 会被记录为链接而不会跟随；
  /// 硬链接会被去重；不支持的文件类型（如设备文件）会被跳过。
  pub fn add_id(
    &mut self,
    root: &Path,
    stack: &mut Vec<(DirId, String, PathBuf, PathBuf)>,
  ) -> Result<()> {
    while let Some((parent, name, source, target)) = stack.pop() {
      let meta = fs::symlink_metadata(&source)?;
      let file_type = meta.file_type();

      #[cfg(unix)]
      let inode_meta = InodeMetadata::from(&meta);
      #[cfg(not(unix))]
      let inode_meta = {
        // 由于需要让 Windows 可执行文件可执行，统一设置可执行权限
        let mut ret = InodeMetadata::new(0o755);
        if let Ok(mtime) = meta.modified() {
          ret.mtime(mtime);
        }
        if let Ok(atime) = meta.accessed() {
          ret.atime(atime);
        }
        ret.ctime(SystemTime::now());
        ret
      };

      #[cfg(windows)]
      let junction = junction::exists(&source).unwrap_or(false);
      #[cfg(not(windows))]
      let junction = false;

      if file_type.is_symlink() || junction {
        let parent_path = source.parent().unwrap_or(Path::new("."));
        let mut link_target = fs::read_link(&source)?.from_absolute(parent_path);
        if link_target.to_relative_limit(root).is_ok() {
          link_target = link_target.to_relative(parent_path)?;
        }
        self
          .builder
          .put_symlink(parent, &name, &inode_meta, &link_target.to_string_lossy())
          .map_err(|e| Error::other(m!(AddSymlinkError, e)))?;
      } else if file_type.is_dir() {
        let dir_id = self
          .builder
          .put_dir(parent, &name, &inode_meta)
          .map_err(|e| Error::other(m!(AddDirError, e)))?;
        self.dirs.insert(target.clone(), dir_id);
        // 子项压栈，代替递归
        for i in fs::read_dir(&source)? {
          let entry = i?;
          let name = entry.file_name();
          stack.push((
            dir_id,
            name.to_string_lossy().to_string(),
            source.join(&name),
            target.join(&name),
          ));
        }
      } else if file_type.is_file() {
        let key = file_id::get_file_id(&source);
        if let Ok(key) = &key
          && let Some(&existing) = self.hardlinks.get(key)
        {
          self
            .builder
            .put_hard_link(parent, &name, existing)
            .map_err(|e| Error::other(m!(AddHardLinkError, e)))?;
          continue;
        }
        let mut f = fs::File::open(&source)?;
        let chunks = self
          .chunker
          .put_reader(&mut f)
          .map_err(|e| Error::other(m!(ReadFileError, e)))?;
        let file_id: FileId = self
          .builder
          .put_file(parent, &name, &inode_meta, chunks)
          .map_err(|e| Error::other(m!(AddFileError, e)))?;
        if let Ok(key) = key {
          self.hardlinks.insert(key, file_id.into());
        }
      } else {
        eprintln!("==> {}: {}", m!(SkipUnsupportedFile), source.display());
      }
    }
    Ok(())
  }

  /// 结束打包：写出剩余块数据与元数据段，并返回最终的写入器
  ///
  /// 调用后不能再添加条目。任一阶段失败时返回 [`Error::other`] 包装的错误。
  pub fn finish(self) -> Result<W> {
    let mut writer = self
      .chunker
      .finish()
      .map_err(|e| Error::other(m!(WriteChunkError, e)))?;
    writer
      .write_metadata_sections(
        &self
          .builder
          .finish()
          .map_err(|e| Error::other(m!(GenMetadataError, e)))?,
        self.compress,
      )
      .map_err(|e| Error::other(m!(WriteMetadataError, e)))?;
    writer
      .finish()
      .map_err(|e| Error::other(m!(WriteDataError, e)))
  }

  /// 确保 `path` 的父目录链在镜像中存在，返回其父目录的 ID
  ///
  /// 不存在的中间目录会以当前时间、`0o755` 权限创建并缓存进 `dirs`；
  /// 根目录（无父目录、空路径、`/`）直接返回根 ID。
  fn create_parent(&mut self, path: &Path) -> Result<DirId> {
    let path = match path.parent() {
      Some(p) => p,
      None => return Ok(self.builder.root()),
    };
    // 没有父目录、空路径或本身就是根（`\`、`/`、盘符等）时直接使用根 ID
    if path.as_os_str().is_empty() || path.file_name().is_none() {
      return Ok(self.builder.root());
    }
    if let Some(&id) = self.dirs.get(path) {
      return Ok(id);
    }

    // 从目标向上收集，直到遇到根或已创建的目录
    let mut to_create: Vec<&Path> = Vec::new();
    let mut current = path;
    let mut base_id = self.builder.root();

    loop {
      // 根目录（`/`、`\`、`C:\`）没有文件名，作为创建的起点而不是目录名
      if current.as_os_str().is_empty() || current.file_name().is_none() {
        base_id = self.builder.root();
        break;
      }
      if let Some(&id) = self.dirs.get(current) {
        base_id = id;
        break;
      }
      to_create.push(current);
      match current.parent() {
        Some(p) => current = p,
        None => break,
      }
    }

    // 逆序遍历：从靠近根的目录开始逐级创建
    let mut parent_id = base_id;
    for &dir_path in to_create.iter().rev() {
      let name = dir_path
        .file_name()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, m!(InvalidDirName)))?
        .to_string_lossy();
      let mut meta = InodeMetadata::new(0o755);
      let time = SystemTime::now();
      meta.mtime(time).atime(time).ctime(time);
      let dir_id = self
        .builder
        .put_dir(parent_id, &name, &meta)
        .map_err(|e| Error::other(m!(AddParentDirError, e)))?;
      self.dirs.insert(dir_path.to_owned(), dir_id);
      parent_id = dir_id;
    }

    Ok(parent_id)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{fs::File, io::Write, path::Component};
  use tempfile::{NamedTempFile, TempDir};

  use crate::dwarfs::Reader;

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

  fn root_metadata() -> InodeMetadata {
    let mut meta = InodeMetadata::new(0o755);
    let now = SystemTime::now();
    meta.mtime(now).atime(now).ctime(now);
    meta
  }

  /// 从空的根目录元数据创建一个空的打包器
  fn empty_packer(output: File) -> Packer<File> {
    Packer::new(root_metadata(), output, CompressParam::Zstd(1)).unwrap()
  }

  /// 读取打包结果，返回解压后的文件树
  ///
  /// 返回的 `TempDir` 需要保持在作用域内，否则解压结果会被一起删除
  fn unpack(image: &NamedTempFile, name: &str) -> (TempDir, PathBuf) {
    let length = image.as_file().metadata().unwrap().len();
    let dir = tempdir();
    let target = dir.path().join(name);
    Reader::new(image.path(), 0, length)
      .unwrap()
      .extract(Path::new(""), &target)
      .unwrap();
    (dir, target)
  }

  fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
  }

  #[test]
  fn add_rejects_duplicate_target_name() {
    let dir = tempdir();
    let source = dir.path().join("a.txt");
    write_file(&source, b"data");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer.add(&source, Path::new("dup.txt")).unwrap();
    // 同一目录内重名会被 dwarfs-enc 拒绝，而不是静默写出两个同名条目
    let err = packer.add(&source, Path::new("dup.txt")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Other);
    // 包装时带上了第三方原始错误，便于定位是哪种冲突
    assert!(err.to_string().contains("DuplicatedEntry"), "err = {err}");
  }

  #[test]
  fn from_rejects_missing_directory() {
    let dir = tempdir();
    let out = NamedTempFile::new().unwrap();
    assert!(
      Packer::from(
        &dir.path().join("missing"),
        out.reopen().unwrap(),
        CompressParam::Zstd(1)
      )
      .is_err()
    );
  }

  #[test]
  fn from_rejects_regular_file() {
    let dir = tempdir();
    let file = dir.path().join("a.txt");
    write_file(&file, b"data");
    let out = NamedTempFile::new().unwrap();
    assert!(Packer::from(&file, out.reopen().unwrap(), CompressParam::Zstd(1)).is_err());
  }

  #[test]
  fn from_packs_directory_tree() {
    let dir = tempdir();
    write_file(&dir.path().join("root.txt"), b"root content");
    write_file(&dir.path().join("sub").join("inner.txt"), b"inner content");

    let out = NamedTempFile::new().unwrap();
    Packer::from(dir.path(), out.reopen().unwrap(), CompressParam::Zstd(1))
      .unwrap()
      .finish()
      .unwrap();
    assert!(out.as_file().metadata().unwrap().len() > 0);

    let (_out_dir, target) = unpack(&out, "tree");
    assert_eq!(read_to_string(&target.join("root.txt")), "root content");
    assert_eq!(
      read_to_string(&target.join("sub").join("inner.txt")),
      "inner content"
    );
  }

  #[test]
  fn from_packs_empty_directory() {
    let dir = tempdir();
    let out = NamedTempFile::new().unwrap();
    Packer::from(dir.path(), out.reopen().unwrap(), CompressParam::Zstd(1))
      .unwrap()
      .finish()
      .unwrap();

    let (_out_dir, target) = unpack(&out, "empty");
    assert!(target.is_dir());
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
  }

  #[test]
  fn add_packs_file_at_archive_root() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"payload");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer.add(&source, Path::new("renamed.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "root");
    assert_eq!(read_to_string(&target.join("renamed.txt")), "payload");
  }

  #[test]
  fn add_creates_intermediate_directories() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"deep");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer.add(&source, Path::new("a/b/c/deep.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "deep");
    assert_eq!(read_to_string(&target.join("a/b/c/deep.txt")), "deep");
  }

  #[test]
  fn add_reuses_existing_parent_directory() {
    let dir = tempdir();
    write_file(&dir.path().join("one.txt"), b"one");
    write_file(&dir.path().join("two.txt"), b"two");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer
      .add(&dir.path().join("one.txt"), Path::new("shared/one.txt"))
      .unwrap();
    packer
      .add(&dir.path().join("two.txt"), Path::new("shared/two.txt"))
      .unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "shared");
    // 两个文件应位于同一个目录，而不是各自新建一个
    assert_eq!(fs::read_dir(target.join("shared")).unwrap().count(), 2);
    assert_eq!(read_to_string(&target.join("shared/one.txt")), "one");
    assert_eq!(read_to_string(&target.join("shared/two.txt")), "two");
  }

  #[test]
  fn add_normalizes_target_path() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"normalized");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    // `./a/../b.txt` 经 clean 后应为 `b.txt`
    packer.add(&source, Path::new("./a/../b.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "normalized");
    assert_eq!(read_to_string(&target.join("b.txt")), "normalized");
  }

  #[test]
  fn add_accepts_absolute_target_path() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"absolute");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    // 镜像内不存在绝对路径，开头的根会被当作镜像根目录处理
    packer.add(&source, Path::new("/abs/nested/a.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "absolute");
    assert_eq!(read_to_string(&target.join("abs/nested/a.txt")), "absolute");
  }

  #[test]
  fn add_accepts_target_path_with_leading_parent_dir() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"escaped");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    // clean 会保留开头的 `..`，但它没有文件名，create_parent 会把它当作根目录的起点，
    // 因此不会跳出镜像根目录
    packer.add(&source, Path::new("../a/b.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "escaped");
    assert_eq!(read_to_string(&target.join("a/b.txt")), "escaped");
  }

  #[test]
  fn add_rejects_invalid_target_name() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"data");
    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    for target in [Path::new(".."), Path::new("")] {
      let err = packer.add(&source, target).err().unwrap();
      assert_eq!(err.kind(), ErrorKind::InvalidInput);
      assert!(err.to_string().contains(m!(InvalidFileName)));
    }
  }

  #[test]
  fn add_rejects_missing_source() {
    let dir = tempdir();
    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    let err = packer
      .add(&dir.path().join("missing.txt"), Path::new("missing.txt"))
      .err()
      .unwrap();
    assert_eq!(err.kind(), ErrorKind::NotFound);
  }

  #[test]
  fn dirs_cache_only_contains_image_paths() {
    let dir = tempdir();
    write_file(&dir.path().join("tree").join("sub").join("a.txt"), b"a");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    // 递归添加：目录由 add_id 登记
    packer
      .add(&dir.path().join("tree"), Path::new("app"))
      .unwrap();
    // 逐级新建：中间目录由 create_parent 登记
    packer
      .add(
        &dir.path().join("tree").join("sub").join("a.txt"),
        Path::new("x/y/b.txt"),
      )
      .unwrap();

    let mut keys: Vec<String> = packer
      .dirs
      .keys()
      .map(|p| p.to_string_lossy().replace('\\', "/"))
      .collect();
    keys.sort();
    // 两条登记路径都必须是镜像内路径，一旦混入文件系统绝对路径就会放错条目
    assert_eq!(keys, ["app", "app/sub", "x", "x/y"]);
  }

  #[test]
  fn add_does_not_confuse_source_and_target_paths() {
    let dir = tempdir();
    let real = dir.path().join("real");
    write_file(&real.join("inner.txt"), b"inner");
    let extra = dir.path().join("extra.txt");
    write_file(&extra, b"extra");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    // 文件系统目录 <tmp>/real 被添加为镜像内的 aliased
    packer.add(&real, Path::new("aliased")).unwrap();
    // 镜像内目标路径刻意取成上面那个文件系统路径：
    // dirs 若混用两套命名空间，这里会误命中 aliased 而把文件放进去
    let target = real.join("extra.txt");
    packer.add(&extra, &target).unwrap();
    packer.finish().unwrap();

    let (_out_dir, root) = unpack(&out, "alias");

    // aliased 只应包含来自 <tmp>/real 的内容
    let mut aliased: Vec<String> = fs::read_dir(root.join("aliased"))
      .unwrap()
      .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
      .collect();
    aliased.sort();
    assert_eq!(aliased, ["inner.txt"]);

    // 绝对目标路径的各级普通组件会在镜像内逐级建出来
    let expected: PathBuf = target
      .components()
      .filter(|c| matches!(c, Component::Normal(_)))
      .collect();
    assert_eq!(read_to_string(&root.join(expected)), "extra");
  }

  #[test]
  fn add_reuses_directory_created_by_recursive_add() {
    let dir = tempdir();
    write_file(&dir.path().join("tree").join("a.txt"), b"a");
    let extra = dir.path().join("extra.txt");
    write_file(&extra, b"extra");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer
      .add(&dir.path().join("tree"), Path::new("app"))
      .unwrap();
    // create_parent 必须复用 add_id 建好的 app，而不是再建一个重名目录
    packer.add(&extra, Path::new("app/extra.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, root) = unpack(&out, "reuse");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    assert_eq!(read_to_string(&root.join("app").join("a.txt")), "a");
    assert_eq!(read_to_string(&root.join("app").join("extra.txt")), "extra");
  }

  #[test]
  fn add_reuses_nested_directory_created_by_from() {
    let dir = tempdir();
    write_file(
      &dir.path().join("sub").join("nested").join("deep.txt"),
      b"deep",
    );
    let extra = dir.path().join("extra.txt");
    write_file(&extra, b"extra");

    let out = NamedTempFile::new().unwrap();
    let mut packer =
      Packer::from(dir.path(), out.reopen().unwrap(), CompressParam::Zstd(1)).unwrap();
    // from 递归建出的深层目录同样以镜像内路径登记，应能被直接复用
    packer
      .add(&extra, Path::new("sub/nested/extra.txt"))
      .unwrap();
    packer.finish().unwrap();

    let (_out_dir, root) = unpack(&out, "nested");
    let nested = root.join("sub").join("nested");
    assert_eq!(read_to_string(&nested.join("deep.txt")), "deep");
    assert_eq!(read_to_string(&nested.join("extra.txt")), "extra");
    // sub 下只应有 nested 一个条目，没有被重建
    assert_eq!(fs::read_dir(root.join("sub")).unwrap().count(), 1);
  }

  #[test]
  fn add_deduplicates_hard_links() {
    let dir = tempdir();
    let source = dir.path().join("source.txt");
    write_file(&source, b"shared content");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer.add(&source, Path::new("first.txt")).unwrap();
    packer.add(&source, Path::new("second.txt")).unwrap();
    packer.add(&source, Path::new("nested/third.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "hardlink");
    assert_eq!(read_to_string(&target.join("first.txt")), "shared content");
    assert_eq!(read_to_string(&target.join("second.txt")), "shared content");
    assert_eq!(
      read_to_string(&target.join("nested/third.txt")),
      "shared content"
    );
  }

  #[test]
  fn add_packs_directory_recursively() {
    let dir = tempdir();
    write_file(&dir.path().join("a.txt"), b"a");
    write_file(&dir.path().join("sub").join("b.txt"), b"b");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer.add(dir.path(), Path::new("tree")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "recursive");
    assert!(target.join("tree").is_dir());
    assert_eq!(read_to_string(&target.join("tree/a.txt")), "a");
    assert_eq!(read_to_string(&target.join("tree/sub/b.txt")), "b");
  }

  #[test]
  fn add_packs_empty_file() {
    let dir = tempdir();
    let source = dir.path().join("empty.txt");
    write_file(&source, b"");

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer.add(&source, Path::new("empty.txt")).unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "emptyfile");
    assert_eq!(read_to_string(&target.join("empty.txt")), "");
  }

  #[test]
  fn finish_produces_readable_archive_without_compression() {
    let dir = tempdir();
    write_file(&dir.path().join("plain.txt"), b"uncompressed");

    let out = NamedTempFile::new().unwrap();
    Packer::from(dir.path(), out.reopen().unwrap(), CompressParam::None)
      .unwrap()
      .finish()
      .unwrap();

    let (_out_dir, target) = unpack(&out, "plain");
    assert_eq!(read_to_string(&target.join("plain.txt")), "uncompressed");
  }

  #[cfg(unix)]
  #[test]
  fn add_packs_symlink() {
    let dir = tempdir();
    write_file(&dir.path().join("real.txt"), b"via symlink");
    std::os::unix::fs::symlink("real.txt", dir.path().join("link.txt")).unwrap();

    let out = NamedTempFile::new().unwrap();
    let mut packer = empty_packer(out.reopen().unwrap());
    packer
      .add(&dir.path().join("link.txt"), Path::new("link.txt"))
      .unwrap();
    packer.finish().unwrap();

    let (_out_dir, target) = unpack(&out, "symlink");
    assert_eq!(read_to_string(&target.join("link.txt")), "via symlink");
  }
}
