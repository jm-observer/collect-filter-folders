use anyhow::{anyhow, bail, Result};
use async_recursion::async_recursion;
use log::debug;
use std::path::PathBuf;
use tokio::fs::read_dir;

const M_SIZE: u64 = 1024 * 1024;
const G_SIZE: u64 = 1024 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    custom_utils::logger::logger_stdout_debug();
    let (path_str, size) = match_command();
    let dir = init_dir(path_str.into()).await?;
    debug!("collected! start to filter");
    let mut results = Vec::new();
    filter(&dir, size, &mut results);
    results.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, s) in &results {
        println!("{}'s size: {}", path, format_size(*s));
    }
    Ok(())
}

fn format_size(size: u64) -> String {
    if size >= G_SIZE {
        format!("{:.2}g", size as f64 / G_SIZE as f64)
    } else if size >= M_SIZE {
        format!("{:.2}m", size as f64 / M_SIZE as f64)
    } else {
        format!("{}b", size)
    }
}

fn filter(dir: &Dir, size: u64, results: &mut Vec<(String, u64)>) -> bool {
    if dir.g_size < size {
        return false;
    }
    let mut is_filter = false;
    for sub in dir.dirs.iter() {
        if filter(sub, size, results) {
            is_filter = true;
        }
    }
    if !is_filter {
        results.push((dir.path.display().to_string(), dir.size));
    }
    true
}

use clap::{Arg, Command};
fn match_command() -> (PathBuf, u64) {
    let matches = Command::new("collect-filter-folders")
        .arg(
            Arg::new("path")
                .short('p')
                .help("扫描的目录路径")
                .default_value(".")
                .takes_value(true),
        )
        .arg(
            Arg::new("size")
                .short('s')
                .help("过滤阈值(GB)")
                .default_value("5")
                .takes_value(true),
        )
        .get_matches();
    let path = matches.value_of("path").unwrap().into();
    let size = matches.value_of("size").unwrap().parse().unwrap();
    (path, size)
}

const SKIP_DIRS: &[&str] = &[
    "$RECYCLE.BIN",
    "System Volume Information",
    "Recovery",
    "$WinREAgent",
];

fn should_skip(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| SKIP_DIRS.iter().any(|s| n.eq_ignore_ascii_case(s)))
        .unwrap_or(false)
}

#[async_recursion]
async fn init_dir(original_path: PathBuf) -> Result<Dir> {
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

    if should_skip(&original_path) {
        debug!("跳过系统目录: {}", original_path.display());
        return Ok(Dir::new(path));
    }

    if path.is_dir() {
        let read_dir_result = read_dir(path.as_path()).await;
        let mut read_dir = match read_dir_result {
            Ok(rd) => rd,
            Err(e) if e.raw_os_error() == Some(5) => {
                debug!("无权限访问, 跳过: {}", path.display());
                return Ok(Dir::new(path));
            }
            Err(e) => {
                return Err(anyhow!("读取文件夹[{}]失败: {}", path.display(), e));
            }
        };
        let mut dir = Dir::new(path.clone());
        let mut dir_res = Vec::default();
        while let Ok(Some(sub_dir)) = read_dir.next_entry().await {
            if let Ok(metadata) = sub_dir.metadata().await {
                if metadata.is_file() {
                    dir.size += metadata.len();
                } else if metadata.is_dir() {
                    dir_res.push(tokio::spawn(
                        async move { init_dir(sub_dir.path()).await },
                    ));
                }
            } else {
                debug!("读取文件[{}]metadata失败", sub_dir.path().display());
            }
        }
        for sub_res in dir_res.into_iter() {
            match sub_res.await.map_err(|e| anyhow!("等待异常: {}", e))? {
                Ok(res) => {
                    if res.size > 0 {
                        dir.add_size(res.size);
                        dir.dirs.push(res);
                    }
                }
                Err(e) => {
                    debug!("{:?}", e);
                }
            }
        }
        Ok(dir)
    } else {
        bail!("文件属性有误或无权限: {}", path.display());
    }
}

#[derive(Debug)]
struct Dir {
    path: PathBuf,
    dirs: Vec<Dir>,
    size: u64,
    g_size: u64,
}

impl Dir {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            dirs: Vec::default(),
            size: 0,
            g_size: 0,
        }
    }
    pub fn add_size(&mut self, size: u64) {
        self.size += size;
        self.g_size = self.size / G_SIZE;
    }
}
