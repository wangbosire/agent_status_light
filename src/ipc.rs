//! 本地 IPC 客户端和消息类型。
//!
//! `send/status/stop` 都通过这里连接 daemon，协议是本地 TCP + JSON Lines。

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    time,
};

use crate::{config::AppPaths, status_priority::PrioritySourceStatus};

#[derive(Debug, Deserialize, Serialize)]
pub struct IpcRequest {
    /// 本机共享 token，避免其它本地进程误发指令。
    pub token: String,
    /// 命令名称，例如 send/status/shutdown。
    pub cmd: String,
    /// send 命令使用的灯效模式。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// send 命令的状态来源，用于 daemon 按 agent 分组做优先级合并。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// send 命令自定义 TTL 秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    /// status 命令是否返回 source/session 明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbose: Option<bool>,
    /// log 命令使用的日志等级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// log 命令使用的事件名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// log 命令使用的日志正文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IpcResponse {
    /// 表示 daemon 是否成功处理请求。
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ble: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<PrioritySourceStatus>>,
}

impl IpcResponse {
    /// 构造一个通用成功响应。
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            daemon: None,
            pid: None,
            ipc: None,
            ble: None,
            device: None,
            mode: None,
            effective_mode: None,
            sources: None,
        }
    }

    /// 构造一个通用失败响应。
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            daemon: None,
            pid: None,
            ipc: None,
            ble: None,
            device: None,
            mode: None,
            effective_mode: None,
            sources: None,
        }
    }
}

/// 向 daemon 发送一条请求，并等待一行 JSON 响应。
pub async fn request(
    paths: &AppPaths,
    cmd: &str,
    mode: Option<&str>,
    source: Option<&str>,
    ttl_seconds: Option<u64>,
    verbose: bool,
    timeout: Duration,
) -> Result<IpcResponse> {
    // 每次请求都读取 token，允许用户删除 runtime 后重新生成 token。
    let token = paths.read_token()?;
    let req = IpcRequest {
        token,
        cmd: cmd.to_owned(),
        mode: mode.map(ToOwned::to_owned),
        source: source.map(ToOwned::to_owned),
        ttl_seconds,
        verbose: verbose.then_some(true),
        level: None,
        event: None,
        message: None,
    };

    send_request(paths, req, timeout).await
}

pub async fn log_event(
    paths: &AppPaths,
    level: impl Into<String>,
    event: impl Into<String>,
    message: impl Into<String>,
) -> Result<IpcResponse> {
    let token = paths.read_token()?;
    let req = IpcRequest {
        token,
        cmd: "log".to_owned(),
        mode: None,
        source: None,
        ttl_seconds: None,
        verbose: None,
        level: Some(level.into()),
        event: Some(event.into()),
        message: Some(message.into()),
    };

    send_request(paths, req, Duration::from_millis(300)).await
}

async fn send_request(paths: &AppPaths, req: IpcRequest, timeout: Duration) -> Result<IpcResponse> {
    let addr = paths.ipc_addr();
    // 外层 timeout 控制整个 IPC 往返时间，避免 Hook 命令卡住。
    let response = time::timeout(timeout, async move {
        let stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("failed to connect to daemon at {addr}"))?;
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // JSON Lines 协议：一条请求以换行结尾，daemon 读取一行后立即响应。
        let mut payload = serde_json::to_vec(&req)?;
        payload.push(b'\n');
        write_half.write_all(&payload).await?;
        write_half.flush().await?;

        // daemon 也返回一行 JSON。这里不做长连接复用，保持客户端极简。
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let response = serde_json::from_str::<IpcResponse>(&line)
            .with_context(|| format!("invalid daemon response: {line:?}"))?;
        Ok::<IpcResponse, anyhow::Error>(response)
    })
    .await
    .context("daemon request timed out")??;

    Ok(response)
}
