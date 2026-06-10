//! 结构化事件日志。
//!
//! 这里记录的是给用户排障看的关键流程日志，不替代 tracing 的调试日志。
//! 文件最多保留最近 1000 条，避免长期运行后无限增长。
//! 写入顺序采用倒序：最新日志在文件最上面，方便直接打开文件时查看最近事件。
//! 写入动作由 daemon 内部的日志 worker 串行执行，因此不需要额外的 events.lock 文件。

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::AppPaths;

const MAX_LOG_ENTRIES: usize = 1000;
const LOG_CHANNEL_CAPACITY: usize = 512;

pub type LogSender = mpsc::Sender<LogEvent>;

#[derive(Debug, Clone)]
pub struct LogEvent {
    pub level: String,
    pub event: String,
    pub message: String,
}

impl LogEvent {
    pub fn new(
        level: impl Into<String>,
        event: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level: level.into(),
            event: event.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogEntry {
    /// 面向用户排障的本地时间，格式为 YYYY-MM-DD HH:mm:ss。
    #[serde(default)]
    pub time: String,
    /// Unix epoch 毫秒时间戳，用于排序和兼容旧日志。
    pub ts_ms: u64,
    /// 日志等级，例如 info/warn/error。
    pub level: String,
    /// 稳定事件名，便于后续按关键字搜索。
    pub event: String,
    /// 面向用户的简短说明。
    pub message: String,
}

/// 创建 daemon 内部的日志队列。
pub fn channel() -> (LogSender, mpsc::Receiver<LogEvent>) {
    mpsc::channel(LOG_CHANNEL_CAPACITY)
}

/// 启动 daemon 内部日志 worker。所有写文件动作都在这个任务里串行执行。
pub fn spawn_writer(
    mut rx: mpsc::Receiver<LogEvent>,
    paths: AppPaths,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut entries = match read_all(&paths) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("warning: failed to load event log cache: {err:#}");
                Vec::new()
            }
        };

        while let Some(event) = rx.recv().await {
            record_to_cache(&mut entries, event);
            if let Err(err) = write_all(&paths, &entries) {
                eprintln!("warning: failed to write event log: {err:#}");
            }
        }
    })
}

/// 非阻塞投递日志。队列满时丢弃日志，避免日志系统反过来阻塞主流程。
pub fn emit(
    tx: &LogSender,
    level: impl Into<String>,
    event: impl Into<String>,
    message: impl Into<String>,
) {
    let _ = tx.try_send(LogEvent::new(level, event, message));
}

fn record_to_cache(entries: &mut Vec<LogEntry>, event: LogEvent) {
    // worker 启动时已经把最近日志读入内存，这里只维护内存缓存。
    // 每次更新后刷新整个小文件，避免追加后再做倒序整理的复杂度。
    let ts_ms = now_ms();
    entries.insert(
        0,
        LogEntry {
            time: format_log_time(ts_ms),
            ts_ms,
            level: event.level,
            event: event.event,
            message: event.message,
        },
    );

    if entries.len() > MAX_LOG_ENTRIES {
        // 文件是倒序排列，头部是最新日志，因此直接截断尾部即可。
        entries.truncate(MAX_LOG_ENTRIES);
    }
}

fn write_all(paths: &AppPaths, entries: &[LogEntry]) -> Result<()> {
    paths.ensure_runtime()?;
    let mut output = String::new();
    for entry in entries {
        output.push_str(&serde_json::to_string(&entry)?);
        output.push('\n');
    }

    fs::write(&paths.events_file, output)
        .with_context(|| format!("failed to write {}", paths.events_file.display()))?;
    Ok(())
}

/// 打印最近的日志，最多展示 1000 条。
pub fn print_recent(limit: usize) -> Result<()> {
    let paths = AppPaths::load()?;
    // 用户传入超过 1000 的 limit 时自动裁剪，避免一次输出过多内容。
    let limit = limit.min(MAX_LOG_ENTRIES);
    let entries = read_recent(&paths, limit)?;

    if entries.is_empty() {
        println!("no logs");
        return Ok(());
    }

    for entry in entries {
        println!(
            "{} [{}] {} - {}",
            entry.time, entry.level, entry.event, entry.message
        );
    }

    Ok(())
}

fn read_recent(paths: &AppPaths, limit: usize) -> Result<Vec<LogEntry>> {
    let entries = read_all(paths)?;
    Ok(entries.into_iter().take(limit).collect())
}

fn read_all(paths: &AppPaths) -> Result<Vec<LogEntry>> {
    if !paths.events_file.exists() {
        // 没有日志文件不是错误，说明 daemon 或相关命令还没产生事件。
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&paths.events_file)
        .with_context(|| format!("failed to read {}", paths.events_file.display()))?;
    let mut entries = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEntry>(line) {
            Ok(mut entry) => {
                if entry.time.is_empty() {
                    entry.time = format_log_time(entry.ts_ms);
                }
                entries.push(entry);
            }
            Err(err) => eprintln!("warning: skipped invalid log line: {err}"),
        }
    }

    // 兼容旧格式或人工编辑过的文件：读取后总是按时间倒序归一化。
    entries.sort_by(|left, right| right.ts_ms.cmp(&left.ts_ms));
    Ok(entries)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn format_log_time(ts_ms: u64) -> String {
    let ts_ms = i64::try_from(ts_ms).unwrap_or(i64::MAX);
    Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .unwrap_or_else(Local::now)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
