#![allow(unused_imports, dead_code)]
use anyhow::{anyhow, bail, Context, Result};
use async_recursion::async_recursion;
use log::{debug, warn};
use std::ffi::OsString;
use std::fs;
use std::future::Future;
use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::fs::{read_dir, DirEntry};

const M_SIZE: u64 = 1024 * 1024;
const G_SIZE: u64 = 1024 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    custom_utils::logger::logger_stdout_debug();
    // let path_str = r"C:\";
    // if path_str.contains("$RECYCLE.BIN") {
    //     bail!("********");
    // }
    let (path_str, size) = match_command();
    let dir = init_dir_v2(path_str.into()).await?;
    // let path = PathBuf::from(path_str);
    // // let path = PathBuf::from(r"D:\$RECYCLE.BIN\S-1-5-18").canonicalize().context(anyhow!("获取[{:?}]绝对路径失败", path_str))?;
    // // println!("start to collect {:?}", path);
    // // let path = PathBuf::from("../");
    // let metadata = path.metadata().context(anyhow!("获取[{:?}]metadata失败", path))?;
    // if metadata.is_file() {
    //     bail!("{:?} is a file not dir", path);
    // }
    // let name = path.file_name().unwrap_or_else(|| path.as_os_str()).to_os_string();
    // let dir = Dir:: new(
    //     name,
    //     path.clone());
    // let dir = init_dir(dir).await?;
    // dir.size = dir.size / 1024 / 1024;
    // debug!("{:?}", dir);
    debug!("colleted! start to filter");
    filter(&dir, size);
    Ok(())
}

fn format_size(size: u64) -> String {
    const M_SIZE: u64 = 1024 * 1024;
    const G_SIZE: u64 = 1024 * 1024 * 1024;
    if size >= G_SIZE {
        format!("{}g", size / G_SIZE)
    } else if size >= M_SIZE {
        format!("{}m", size / M_SIZE)
    } else {
        format!("{}b", size)
    }
}

fn filter(dir: &Dir, size: u64) -> bool {
    if dir.g_size < size {
        return false;
    }
    let mut is_filter = false;
    for sub in dir.dirs.iter() {
        if filter(sub, size) {
            is_filter = true;
        }
    }
    if is_filter == false {
        debug!("{}'s size: {}", dir.path.display(), format_size(dir.size));
    }
    return true;
}

use clap::{Arg, Command};
fn match_command() -> (PathBuf, u64) {
    let matches = Command::new("pacman")
        .arg(
            Arg::new("path")
                .short('p')
                .default_value(".")
                .takes_value(true),
        )
        .arg(
            Arg::new("size")
                .short('s')
                .default_value("5")
                .takes_value(true),
        )
        .get_matches();
    let path = matches.value_of("path").unwrap().into();
    let size = matches.value_of("size").unwrap().parse().unwrap();
    (path, size)
}

#[async_recursion]
async fn init_dir(mut dir: Dir) -> Result<Dir> {
    let path = dir.path.clone();
    let mut read_dir = read_dir(path.as_path())
        .await
        .context(anyhow!("读取文件夹[{:?}]失败", path.as_path()))?;
    let mut dir_res = Vec::default();
    while let Ok(Some(sub_dir)) = read_dir.next_entry().await {
        // println!("{:?} {} {:?}", sub_dir.file_name(), sub_dir.metadata().await?.is_dir(), sub_dir.path());
        if let Ok(metadata) = sub_dir.metadata().await {
            if metadata.is_file() {
                dir.size += metadata.len();
                // dir.files.push(File {
                //     name: sub_dir.file_name(), path: sub_dir.path(), size: metadata.len()
                // })
            } else if metadata.is_dir() {
                let sub_dir = Dir::new(sub_dir.file_name(), sub_dir.path());
                dir_res.push(tokio::spawn(async move { init_dir(sub_dir).await }));
            }
        } else {
            warn!("读取文件[{}]metadata失败", sub_dir.path().display());
        }
    }
    for sub_res in dir_res.into_iter() {
        let res = sub_res.await.context(anyhow!("等待异常"))??;
        if res.size > 0 {
            dir.add_size(res.size);
            dir.dirs.push(res);
        }
    }
    Ok(dir)
}

#[async_recursion]
async fn init_dir_v2(original_path: PathBuf) -> Result<Dir> {
    // Normalize Windows path to support extended length paths
    #[cfg(target_os = "windows")]
    fn normalize_path(p: &std::path::Path) -> std::path::PathBuf {
        let p_str = p.display().to_string();
        if p_str.starts_with(r"\\?\\") {
            return p.to_path_buf();
        }
        let mut norm = std::path::PathBuf::from(r"\\?\\");
        norm.push(p);
        norm
    }
    #[cfg(not(target_os = "windows"))]
    fn normalize_path(p: &std::path::Path) -> std::path::PathBuf {
        p.to_path_buf()
    }

    let path = normalize_path(&original_path);
    // rest of the function uses `path` variable

    // let path = dir.path.clone();
    if path.is_dir() {
        let mut read_dir = read_dir(path.as_path())
            .await
            .context(anyhow!("读取文件夹[{}]失败", path.as_path().display()))?;
        let name = path
            .file_name()
            .unwrap_or_else(|| path.as_os_str())
            .to_os_string();
        let mut dir = Dir::new(name, path.clone());
        let mut dir_res = Vec::default();
        while let Ok(Some(sub_dir)) = read_dir.next_entry().await {
            // println!("{:?} {} {:?}", sub_dir.file_name(), sub_dir.metadata().await?.is_dir(), sub_dir.path());
            if let Ok(metadata) = sub_dir.metadata().await {
                if metadata.is_file() {
                    dir.size += metadata.len();
                    // dir.files.push(File {
                    //     name: sub_dir.file_name(), path: sub_dir.path(), size: metadata.len()
                    // })
                } else if metadata.is_dir() {
                    // let sub_dir = Dir::new(
                    //     sub_dir.file_name(),
                    //     sub_dir.path(),);
                    // dbg!("{:?}", sub_dir.path());
                    dir_res.push(tokio::spawn(
                        async move { init_dir_v2(sub_dir.path()).await },
                    ));
                }
            } else {
                warn!("读取文件[{}]metadata失败", sub_dir.path().display());
            }
        }
        for sub_res in dir_res.into_iter() {
            match sub_res.await.context(anyhow!("等待异常"))? {
                Ok(res) => {
                    if res.size > 0 {
                        dir.add_size(res.size);
                        dir.dirs.push(res);
                    }
                }
                Err(e) => {
                    warn!("{:?}", e);
                }
            }
        }
        Ok(dir)
    } else {
        bail!("文件属性有误或无权限: {}", path.display());
    }
}

#[derive(Debug)]
struct File {
    name: OsString,
    path: PathBuf,
    size: u64,
    m_size: u64,
}
#[derive(Debug)]
struct Dir {
    name: OsString,
    path: PathBuf,
    files: Vec<File>,
    dirs: Vec<Dir>,
    size: u64,
    m_size: u64,
    g_size: u64,
}

impl File {
    pub fn new(name: impl Into<OsString>, path: impl Into<PathBuf>, size: u64) -> Self {
        let m_size = size / M_SIZE;
        Self {
            name: name.into(),
            path: path.into(),
            size,
            m_size,
        }
    }
    #[inline]
    pub fn size(&self) -> u64 {
        self.size
    }
    #[inline]
    pub fn m_size(&self) -> u64 {
        self.m_size
    }
}
impl Dir {
    pub fn new(name: impl Into<OsString>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            files: Vec::default(),
            dirs: Vec::default(),
            size: 0,
            m_size: 0,
            g_size: 0,
        }
    }
    pub fn add_size(&mut self, size: u64) {
        self.size += size;
        self.m_size = self.size / M_SIZE;
        self.g_size = self.size / G_SIZE;
    }
    #[inline]
    pub fn size(&self) -> u64 {
        self.size
    }
    #[inline]
    pub fn m_size(&self) -> u64 {
        self.m_size
    }
    #[inline]
    pub fn g_size(&self) -> u64 {
        self.g_size
    }
}

#[test]
fn test() {
    let path = PathBuf::from("C:\\Windows\\System32\\sru");
    let metadata = path.metadata().unwrap();
    println!(
        "{} {:?}",
        metadata.file_attributes(),
        metadata.permissions()
    );
}
