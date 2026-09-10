use std::{
  env,
  fs::{self, File},
  io::{Error, ErrorKind, Result},
  os::windows::process::CommandExt,
  path::{Path, PathBuf},
  process::Command,
};

use crate::{
  include_lang, okcancel,
  payload::{LauncherPayload, WritableType},
  retry, retryclose, tempname,
  win::{
    depend::{DependPath, check_depend, extract_depend, install_depend, uninstall_depend},
    dialog,
    launch::{Depend, LauncherError, Program, run},
  },
  yesno, yesnocancel,
};
use tempfile::TempDir;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

include_lang!();

/// 应用镜像启动器，负责准备运行环境并启动镜像中的程序
pub struct Launcher<'a> {
  /// 自身可执行文件的路径
  path: &'a Path,
  /// 自身可执行文件的句柄，用于读取内嵌的依赖与镜像
  file: &'a mut File,
  /// 从 `.payload` 段读出的启动信息
  payload: &'a LauncherPayload,
  /// 临时目录，用于存放依赖、挂载点与可写层
  tempdir: &'a TempDir,
  /// 本次运行需要启动的依赖进程
  depend: Vec<Depend>,
}

impl Launcher<'_> {
  /// 创建一个启动器
  ///
  /// `tempdir` 需要在整个运行期间保持存活，否则临时文件会被提前删除。
  pub fn from<'a>(
    path: &'a Path,
    file: &'a mut File,
    payload: &'a LauncherPayload,
    tempdir: &'a TempDir,
  ) -> Launcher<'a> {
    Launcher {
      path,
      file,
      payload,
      tempdir,
      depend: Vec::new(),
    }
  }

  /// 准备环境并启动应用，失败时弹窗询问是否重启启动流程
  ///
  /// 返回应用的退出码；用户放弃时返回 `u32::MAX`。
  pub fn start(&mut self) -> u32 {
    loop {
      match self.prepare().and_then(|path| self.launch(&path)) {
        Ok(Some(code)) => return code,
        Ok(None) => {}
        Err(err) => {
          if !retry!(m!(LaunchErrorTitle), &err.to_string(), &format!("{err:?}")) {
            return u32::MAX;
          };
        }
      }
    }
  }

  /// 启动 `app_path` 中的应用，并追加运行时传入的命令行参数
  ///
  /// `app_path` 用于解析相对路径形式的 `exe_path` 与 `exe_workdir`。
  /// 返回 `Ok(Some(退出码))` 表示应用已结束；`Ok(None)` 表示需要重试整个启动流程；
  /// `Err` 表示启动失败并进入错误处理。
  pub fn launch(&mut self, app_path: &Path) -> Result<Option<u32>> {
    // 外部程序与绝对路径原样使用，其余相对镜像挂载点解析
    let exe_path = if self.payload.exe_external || Path::new(&self.payload.exe_path).is_absolute() {
      self.payload.exe_path.clone()
    } else {
      app_path
        .join(&self.payload.exe_path)
        .to_string_lossy()
        .to_string()
    };

    let mut exe_args: Vec<String> = self
      .payload
      .exe_args
      .iter()
      .map(|s| s.to_string())
      .collect();
    exe_args.extend(env::args().skip(1));

    let exe_workdir = self.payload.exe_workdir.as_ref().map(|p| {
      {
        let path = PathBuf::from(p);
        if path.is_absolute() {
          path
        } else {
          app_path.join(path)
        }
      }
      .to_string_lossy()
      .to_string()
    });

    match run(
      &Program {
        path: exe_path,
        args: exe_args,
        workdir: exe_workdir,
      },
      &self.depend,
      self.payload.exe_started_time,
      self.payload.exe_console,
    ) {
      Ok(code) => Ok(Some(code)),
      Err(err) => self.error(&err),
    }
  }

  /// 解压依赖并调用安装脚本，成功时返回依赖路径，用户取消或最终失败时返回 `None`
  ///
  /// 依赖包为空时会引导用户手动下载安装，安装成功前会一直循环提示，
  /// 因此这里用循环而不是递归，避免用户反复重试导致栈溢出。
  fn install(&mut self) -> Option<DependPath> {
    loop {
      let depend_path = self.tempdir.path().join(tempname(6));
      match extract_depend(self.path, self.file, &self.payload.depend, &depend_path) {
        Some(true) => return install_depend(&depend_path),
        // 依赖已由用户手动安装，重新检查一次；仍未安装则回到循环继续引导
        Some(false) => {
          if let Ok(p) = check_depend(true) {
            return Some(p);
          }
        }
        None => return None,
      }
    }
  }

  /// 解压依赖并调用卸载脚本，返回依赖是否已不再处于安装状态
  ///
  /// 与 [`Launcher::install`] 一样，依赖包为空时会循环提示，不使用递归。
  fn uninstall(&mut self) -> bool {
    loop {
      let depend_path = self.tempdir.path().join(tempname(6));
      match extract_depend(self.path, self.file, &self.payload.depend, &depend_path) {
        Some(true) => return uninstall_depend(&depend_path),
        Some(false) => {
          if check_depend(true).is_err() {
            return true;
          }
        }
        None => return false,
      }
    }
  }

  /// 兼容模式：不使用文件系统挂载，直接把镜像解压到临时目录
  ///
  /// 会清空已准备的依赖列表，返回解压后的应用根目录。
  fn compat(&mut self) -> Result<PathBuf> {
    self.depend.clear();
    let temp_path = self.tempdir.path().join(tempname(6));
    self
      .payload
      .image
      .extract(self.path, self.file, &temp_path)?;
    Ok(temp_path)
  }

  /// 准备运行环境：校验依赖并组装本次要启动的依赖进程列表
  ///
  /// 返回应用的可见根目录：未启用可写层时为 DwarFS 挂载点，否则为 OverlayFS 挂载点。
  /// 依赖缺失且用户既不安装也不使用兼容模式时返回 `ErrorKind::PermissionDenied`。
  fn prepare(&mut self) -> Result<PathBuf> {
    let check_ovlfs = self.payload.exe_writable != WritableType::None;
    let depend_path = match check_depend(check_ovlfs) {
      Ok(p) => p,
      Err(err) => {
        if okcancel!(
          m!(InstallDependTitle),
          m!(InstallDependPrompt),
          &m!(InstallDependPromptContent, err)
        ) && let Some(p) = self.install()
        {
          p
        } else if okcancel!(
          m!(CompatModeTitle),
          m!(CompatModePrompt),
          m!(CompatModeContent)
        ) {
          return self.compat();
        } else {
          return Err(Error::new(ErrorKind::PermissionDenied, m!(UserCancelled)));
        }
      }
    };

    let workdir = Some(self.tempdir.path().to_string_lossy().to_string());
    let dwarfs_path_name = tempname(6);
    let dwarfs_path = self.tempdir.path().join(&dwarfs_path_name);

    self.depend.clear();
    self.depend.push(Depend {
      name: "DwarFS".to_string(),
      target: dwarfs_path.clone(),
      program: Program {
        path: depend_path.dwarfs_path.to_string_lossy().to_string(),
        args: vec![
          self.path.to_string_lossy().to_string(),
          dwarfs_path_name.clone(),
          "-o".to_string(),
          format!("offset={}", self.payload.image.offset),
          "-o".to_string(),
          format!("imagesize={}", self.payload.image.length),
        ],
        workdir: workdir.clone(),
      },
    });

    let path = match self.payload.exe_writable {
      WritableType::Save => {
        let name = self
          .path
          .with_extension("data")
          .file_name()
          .map(|s| s.to_string_lossy().to_string())
          .unwrap_or("data".to_string());
        let path = self.path.parent().unwrap_or(Path::new(".")).join(name);
        if !path.exists() {
          fs::create_dir(&path)?
        }
        path.to_string_lossy().to_string()
      }

      WritableType::Temp => {
        let name = tempname(6);
        let path = self.tempdir.path().join(&name);
        fs::create_dir(&path)?;
        name
      }

      WritableType::Memory => {
        let name = tempname(6);
        let path = self.tempdir.path().join(&name);
        self.depend.push(Depend {
          name: "MemFS".to_string(),
          target: path.clone(),
          program: Program {
            path: depend_path.memfs_path.to_string_lossy().to_string(),
            args: vec!["-m".to_string(), name.clone()],
            workdir: workdir.clone(),
          },
        });
        name
      }

      WritableType::None => return Ok(dwarfs_path),
    };

    let ovlfs_path_name = tempname(6);
    let ovlfs_path = self.tempdir.path().join(&ovlfs_path_name);

    self.depend.push(Depend {
      name: "OverlayFS".to_string(),
      target: ovlfs_path.clone(),
      program: Program {
        path: depend_path.ovlfs_path.to_string_lossy().to_string(),
        args: vec![
          "-l".to_string(),
          dwarfs_path_name,
          "-u".to_string(),
          path,
          "-m".to_string(),
          ovlfs_path_name,
        ],
        workdir: workdir.clone(),
      },
    });

    Ok(ovlfs_path)
  }

  /// 处理启动错误：先校验镜像完整性，再让用户选择修复 / 重启 / 终止
  ///
  /// 镜像校验失败直接返回 `ErrorKind::InvalidData`。
  /// 返回 `Ok(None)` 表示重启整个启动流程，`Ok(Some(退出码))` 表示按该码退出。
  fn error(&mut self, err: &LauncherError) -> Result<Option<u32>> {
    if !self.payload.image.hash(self.file)? {
      return Err(Error::new(ErrorKind::InvalidData, m!(DataVerifyFailed)));
    }
    match retryclose!(
      m!(LaunchErrorPromptTitle),
      &err.to_string(),
      m!(LaunchErrorPromptContent)
    ) {
      Some(true) => self.repair(),
      Some(false) => Ok(None),
      None => Ok(Some(err.code())),
    }
  }

  /// 修复启动错误：重装依赖、彻底重装依赖或使用兼容模式
  ///
  /// 返回 `Ok(None)` 表示重启整个启动流程，`Ok(Some(退出码))` 表示按该码退出。
  /// 选择“彻底重装依赖”时会在卸载完成后提示重启系统，并以退出码 1 结束。
  fn repair(&mut self) -> Result<Option<u32>> {
    loop {
      match yesnocancel!(m!(RepairTitle), m!(RepairPrompt), m!(RepairContent)) {
        Some(true) => {
          if self.install().is_some() {
            return Ok(None);
          }
        }
        Some(false) => {
          if self.uninstall() {
            if yesno!(
              m!(RepairTitle),
              m!(DependUninstallComplete),
              m!(RestartSystemPrompt)
            ) {
              let _ = Command::new("shutdown")
                .args(["/r", "/t", "5"])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
            }
            return Ok(Some(1));
          }
        }
        None => {
          let app_path = self.compat()?;
          return self.launch(&app_path);
        }
      }
    }
  }
}
