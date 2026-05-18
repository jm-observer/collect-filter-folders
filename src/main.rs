use anyhow::{anyhow, bail, Result};
use async_recursion::async_recursion;
use chrono::{DateTime, Duration, Local};
use log::debug;
use std::path::{Path, PathBuf};
use tokio::fs::read_dir;

const M_SIZE: u64 = 1024 * 1024;
const G_SIZE: u64 = 1024 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    custom_utils::logger::logger_stdout_debug();

    let cli = build_cli().get_matches();

    match cli.subcommand() {
        Some(("scan", sub)) => {
            let path: PathBuf = sub.value_of("path").unwrap().into();
            let size: u64 = sub.value_of("size").unwrap().parse().unwrap();
            cmd_scan(path, size).await?;
        }
        Some(("clean-rust", sub)) => {
            let path: PathBuf = sub.value_of("path").unwrap().into();
            let days: i64 = sub.value_of("days").unwrap().parse().unwrap();
            let dry_run = sub.is_present("dry-run");
            cmd_clean_rust(path, days, dry_run).await?;
        }
        _ => {
            // 向后兼容：无子命令时按旧逻辑处理
            let path: PathBuf = cli.value_of("path").unwrap_or(".").into();
            let size: u64 = cli.value_of("size").unwrap_or("5").parse().unwrap();
            cmd_scan(path, size).await?;
        }
    }

    Ok(())
}

// ==================== CLI ====================

use clap::{Arg, Command};

fn build_cli() -> Command<'static> {
    Command::new("collect-filter-folders")
        .about("磁盘空间分析与清理工具")
        .long_about(
            "磁盘空间分析与清理工具\n\n\
             功能:\n  \
             - 扫描目录树, 找出占用空间超过阈值的大文件夹\n  \
             - 清理Rust项目中长期未使用的编译产物\n\n\
             示例:\n  \
             collect-filter-folders scan -p D:\\ -s 10\n  \
             collect-filter-folders clean-rust -p D:\\ -d 14 --dry-run",
        )
        .subcommand_required(false)
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
        .subcommand(
            Command::new("scan")
                .about("扫描目录, 找出超过指定大小的文件夹并按大小降序输出")
                .long_about(
                    "递归扫描指定目录, 找出占用空间超过阈值的文件夹, 按大小从大到小排序输出。\n\n\
                     示例:\n  \
                     scan -p D:\\ -s 10    # 扫描D盘, 列出超过10GB的目录",
                )
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
                        .help("过滤阈值, 单位GB")
                        .default_value("5")
                        .takes_value(true),
                ),
        )
        .subcommand(
            Command::new("clean-rust")
                .about("清理Rust项目中过期的target/debug和target/release目录")
                .long_about(
                    "递归扫描目录, 通过Cargo.toml识别Rust项目, 检查target/debug和target/release的修改时间。\n\
                     超过指定天数未修改的编译产物将被清理, 释放磁盘空间。\n\n\
                     示例:\n  \
                     clean-rust -p D:\\ -d 14          # 清理D盘下超过14天的编译产物\n  \
                     clean-rust -p D:\\ --dry-run      # 仅预览, 不实际删除",
                )
                .arg(
                    Arg::new("path")
                        .short('p')
                        .help("扫描的磁盘或目录路径")
                        .default_value(".")
                        .takes_value(true),
                )
                .arg(
                    Arg::new("days")
                        .short('d')
                        .help("超过多少天未修改则清理")
                        .default_value("7")
                        .takes_value(true),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .help("仅预览将要删除的目录, 不实际删除(推荐先用此参数确认)")
                        .takes_value(false),
                ),
        )
}

// ==================== scan 命令 ====================

async fn cmd_scan(path: PathBuf, size: u64) -> Result<()> {
    let dir = init_dir(path.into()).await?;
    debug!("collected! start to filter");
    let mut results = Vec::new();
    filter(&dir, size, &mut results);
    results.sort_by(|a, b| b.1.cmp(&a.1));
    println!();
    for (path, s) in &results {
        println!("\t{}'s size: {}", path, format_size(*s));
    }
    println!();
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

// ==================== clean-rust 命令 ====================

async fn cmd_clean_rust(path: PathBuf, days: i64, dry_run: bool) -> Result<()> {
    let mut targets = Vec::new();
    find_rust_targets(&path, days, &mut targets).await?;

    if targets.is_empty() {
        println!("未发现需要清理的Rust target目录");
        return Ok(());
    }

    // 按大小降序排序
    targets.sort_by(|a, b| b.size.cmp(&a.size));

    let mut total_size: u64 = 0;
    let mut cleaned_count: u32 = 0;

    for target in &targets {
        let action = if dry_run { "[预览]" } else { "[删除]" };
        println!(
            "{} {} (大小: {}, 最后修改: {})",
            action,
            target.path.display(),
            format_size(target.size),
            target.modified.format("%Y-%m-%d %H:%M")
        );

        if !dry_run {
            match tokio::fs::remove_dir_all(&target.path).await {
                Ok(_) => {
                    total_size += target.size;
                    cleaned_count += 1;
                }
                Err(e) => {
                    println!("  删除失败: {}", e);
                }
            }
        } else {
            total_size += target.size;
            cleaned_count += 1;
        }
    }

    let summary = if dry_run { "预计可清理" } else { "已清理" };
    println!(
        "\n{}: {} 个目录, 共 {}",
        summary,
        cleaned_count,
        format_size(total_size)
    );

    Ok(())
}

struct CleanTarget {
    path: PathBuf,
    size: u64,
    modified: DateTime<Local>,
}

/// 递归扫描目录, 找到 Rust 项目 (含 Cargo.toml) 中过期的 target/debug 和 target/release
#[async_recursion]
async fn find_rust_targets(dir: &Path, days: i64, targets: &mut Vec<CleanTarget>) -> Result<()> {
    if should_skip(dir) {
        return Ok(());
    }

    let read_dir_result = read_dir(dir).await;
    let mut rd = match read_dir_result {
        Ok(rd) => rd,
        Err(e) if e.raw_os_error() == Some(5) => {
            debug!("无权限访问, 跳过: {}", dir.display());
            return Ok(());
        }
        Err(e) => {
            debug!("读取目录失败[{}]: {}", dir.display(), e);
            return Ok(());
        }
    };

    // 先收集所有条目
    let mut entries = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        entries.push(entry);
    }

    // 判断是否是 Rust 项目
    let has_cargo_toml = entries.iter().any(|e| e.file_name() == "Cargo.toml");

    if has_cargo_toml {
        let target_dir = dir.join("target");
        if target_dir.is_dir() {
            let threshold = Local::now() - Duration::days(days);
            for sub in &["debug", "release"] {
                let sub_path = target_dir.join(sub);
                if sub_path.is_dir() {
                    if let Ok(modified) = get_latest_modified(&sub_path).await {
                        if modified < threshold {
                            let size = dir_size(&sub_path).await.unwrap_or(0);
                            targets.push(CleanTarget {
                                path: sub_path,
                                size,
                                modified,
                            });
                        } else {
                            debug!(
                                "跳过(未过期): {} (最后修改: {})",
                                sub_path.display(),
                                modified.format("%Y-%m-%d %H:%M")
                            );
                        }
                    }
                }
            }
        }
        // Rust 项目内部不再递归（target 内不会有其他 Rust 项目）
        // 但工作空间下的子 crate 可能有独立 Cargo.toml，继续递归非 target 目录
        for entry in &entries {
            if let Ok(ft) = entry.file_type().await {
                if ft.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    // 跳过 target 和隐藏目录
                    if name_str != "target" && !name_str.starts_with('.') {
                        find_rust_targets(&entry.path(), days, targets).await?;
                    }
                }
            }
        }
    } else {
        // 不是 Rust 项目，继续递归子目录
        for entry in &entries {
            if let Ok(ft) = entry.file_type().await {
                if ft.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !name_str.starts_with('.') {
                        find_rust_targets(&entry.path(), days, targets).await?;
                    }
                }
            }
        }
    }

    Ok(())
}

/// 获取目录下最新的修改时间（递归检查目录自身的 metadata）
async fn get_latest_modified(path: &Path) -> Result<DateTime<Local>> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata.modified()?;
    Ok(DateTime::<Local>::from(modified))
}

/// 递归计算目录大小
#[async_recursion]
async fn dir_size(path: &Path) -> Result<u64> {
    let mut total: u64 = 0;
    let mut rd = read_dir(path).await?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        if let Ok(metadata) = entry.metadata().await {
            if metadata.is_file() {
                total += metadata.len();
            } else if metadata.is_dir() {
                total += dir_size(&entry.path()).await.unwrap_or(0);
            }
        }
    }
    Ok(total)
}

// ==================== scan 用的目录树 ====================

const SKIP_DIRS: &[&str] = &[
    "$RECYCLE.BIN",
    "System Volume Information",
    "Recovery",
    "$WinREAgent",
];

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| SKIP_DIRS.iter().any(|s| n.eq_ignore_ascii_case(s)))
        .unwrap_or(false)
}

#[async_recursion]
async fn init_dir(original_path: PathBuf) -> Result<Dir> {
    #[cfg(target_os = "windows")]
    fn normalize_path(p: &Path) -> PathBuf {
        let p_str = p.display().to_string();
        if p_str.starts_with(r"\\?\\") {
            return p.to_path_buf();
        }
        let mut norm = PathBuf::from(r"\\?\\");
        norm.push(p);
        norm
    }
    #[cfg(not(target_os = "windows"))]
    fn normalize_path(p: &Path) -> PathBuf {
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
