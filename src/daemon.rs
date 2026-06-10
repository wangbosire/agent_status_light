//! daemon 生命周期和用户命令实现。
//!
//! daemon 是唯一直接持有 BLE 连接的进程；`send` 只是一个很薄的 IPC 客户端。
//! 这样 Hook 触发时只需要做一次本地 IPC，不会每次都重新扫描和连接蓝牙。
//!
//! 主要职责边界：
//! - `send_mode`：校验 mode、尝试连接 daemon、必要时自动拉起 daemon。
//! - `run_foreground`：真正的 daemon 主循环，负责 IPC server 和 BLE worker。
//! - `handle_client`：处理单个 JSON Lines IPC 请求。
//! - `stop` / `force_stop`：提供后台进程的优雅停止和兜底停止。

use std::{
    fs::OpenOptions,
    io::{IsTerminal, Read},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{RwLock, mpsc, watch},
    time,
};
use tracing::{info, warn};

use crate::{
    ble::{self, BleSnapshot, SharedBleStatus},
    config::AppPaths,
    ipc::{self, IpcRequest, IpcResponse},
    log_store::{self, LogSender},
    modes,
    status_priority::{self, DEFAULT_SESSION, MANUAL_SOURCE, StatusPriority},
};

/// 根据参数选择后台启动或前台运行。
pub async fn run(foreground: bool) -> Result<()> {
    if foreground {
        run_foreground().await
    } else {
        start_background().await
    }
}

pub async fn send_mode(
    mode: &str,
    source: &str,
    session: &str,
    ttl_seconds: Option<u64>,
    quiet: bool,
    strict: bool,
) -> Result<()> {
    // 先在 CLI 客户端侧做 mode 校验，避免无效请求进入 IPC。
    let Some(mode) = modes::normalize_mode(mode) else {
        bail!(
            "invalid mode {mode:?}; valid modes: {}",
            modes::valid_modes_csv()
        );
    };
    let source_key = resolve_source_key(source, session);
    let ttl_seconds = normalize_ttl_seconds(ttl_seconds)?;

    let paths = AppPaths::load()?;
    paths.ensure_token()?;

    // 第一优先级是复用已有 daemon。这个路径最快，适合高频 Hook 调用。
    match ipc::request(
        &paths,
        "send",
        Some(&mode),
        Some(&source_key),
        ttl_seconds,
        false,
        Duration::from_secs(1),
    )
    .await
    {
        Ok(response) if response.ok => {
            return Ok(());
        }
        Ok(response) => {
            if strict {
                bail!(
                    "{}",
                    response.error.unwrap_or_else(|| "send failed".to_owned())
                );
            }
        }
        Err(err) => {
            // daemon 不存在或暂时不可达时，send 会尝试自动拉起 daemon。
            warn!("daemon unavailable, attempting to start it: {err:#}");
        }
    }

    // 自动拉起后只重试一次。默认失败只 warning，避免打断 agent 主流程；
    // 用户显式传入 --strict 时，失败才作为命令错误返回。
    if let Err(err) = start_background().await {
        if strict {
            return Err(err);
        }
        warn_if_needed(
            quiet,
            format!("failed to start AgentStatusLight daemon: {err:#}"),
        );
        return Ok(());
    }

    match ipc::request(
        &paths,
        "send",
        Some(&mode),
        Some(&source_key),
        ttl_seconds,
        false,
        Duration::from_secs(2),
    )
    .await
    {
        Ok(response) if response.ok => Ok(()),
        Ok(response) if strict => bail!(
            "{}",
            response.error.unwrap_or_else(|| "send failed".to_owned())
        ),
        Ok(response) => {
            warn_if_needed(
                quiet,
                format!(
                    "failed to send mode to AgentStatusLight: {}",
                    response.error.unwrap_or_else(|| "unknown error".to_owned())
                ),
            );
            Ok(())
        }
        Err(err) if strict => Err(err),
        Err(err) => {
            warn_if_needed(
                quiet,
                format!("failed to send mode to AgentStatusLight: {err:#}"),
            );
            Ok(())
        }
    }
}

fn warn_if_needed(quiet: bool, message: String) {
    if !quiet {
        eprintln!("warning: {message}");
    }
}

fn normalize_ttl_seconds(ttl_seconds: Option<u64>) -> Result<Option<u64>> {
    match ttl_seconds {
        Some(0) => bail!("ttl must be greater than 0 seconds"),
        Some(seconds) if seconds > 24 * 60 * 60 => {
            bail!("ttl must be less than or equal to 86400 seconds")
        }
        value => Ok(value),
    }
}

fn resolve_source_key(source: &str, session: &str) -> String {
    let session = if session.trim().eq_ignore_ascii_case("auto") {
        read_auto_session().unwrap_or_else(|| DEFAULT_SESSION.to_owned())
    } else {
        status_priority::normalize_session(session)
    };

    status_priority::source_key(source, &session)
}

fn read_auto_session() -> Option<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }

    let mut raw = String::new();
    stdin.lock().read_to_string(&mut raw).ok()?;
    extract_session_from_hook_input(&raw)
}

fn extract_session_from_hook_input(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    find_string_field(
        &value,
        &[
            "session_id",
            "sessionId",
            "conversation_id",
            "conversationId",
            "thread_id",
            "threadId",
            "chat_id",
            "chatId",
            "request_session_id",
            "requestSessionId",
            "tab_id",
            "tabId",
            "workspace_id",
            "workspaceId",
            "project_id",
            "projectId",
        ],
    )
    .map(status_priority::normalize_session)
    .or_else(|| {
        find_string_field(
            &value,
            &[
                "cwd",
                "workspace",
                "workspace_path",
                "workspacePath",
                "project_path",
                "projectPath",
                "repo_path",
                "repoPath",
                "root",
                "root_path",
                "rootPath",
                "transcript_path",
                "transcriptPath",
            ],
        )
        .map(|stable_text| format!("hash-{}", stable_hash(stable_text)))
    })
}

fn find_string_field<'a>(value: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    match value {
        serde_json::Value::Object(object) => {
            for name in names {
                if let Some(candidate) = object.get(*name).and_then(serde_json::Value::as_str)
                    && !candidate.trim().is_empty()
                {
                    return Some(candidate);
                }
            }

            object
                .values()
                .find_map(|child| find_string_field(child, names))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|child| find_string_field(child, names)),
        _ => None,
    }
}

fn stable_hash(value: &str) -> String {
    // FNV-1a 64-bit：简单、稳定、跨 Rust 版本不会改变，适合作为 fallback session key。
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub async fn print_status(verbose: bool) -> Result<()> {
    let paths = AppPaths::load()?;
    paths.ensure_token()?;

    // status 不负责启动 daemon；连接不上 IPC 就认为后台服务没有运行。
    match ipc::request(
        &paths,
        "status",
        None,
        None,
        None,
        verbose,
        Duration::from_secs(1),
    )
    .await
    {
        Ok(response) if response.ok => {
            println!(
                "daemon: {}",
                response.daemon.unwrap_or_else(|| "running".to_owned())
            );
            if let Some(pid) = response.pid {
                println!("pid: {pid}");
            }
            if let Some(ipc) = response.ipc {
                println!("ipc: {ipc}");
            }
            if let Some(ble) = response.ble {
                println!("ble: {ble}");
            }
            if let Some(device) = response.device {
                println!("device: {device}");
            }
            if let Some(mode) = response.mode {
                println!("mode: {mode}");
            }
            if verbose && let Some(effective_mode) = response.effective_mode {
                println!("effective: {effective_mode}");
            }
            if verbose && let Some(sources) = response.sources {
                if sources.is_empty() {
                    println!("sources: none");
                } else {
                    println!("sources:");
                    for source in sources {
                        println!(
                            "  {} mode={} priority={} expires_in={}s",
                            source.source, source.mode, source.priority, source.expires_in_secs
                        );
                    }
                }
            }
        }
        _ => {
            println!("daemon: stopped");
        }
    }

    Ok(())
}

pub async fn stop(force: bool) -> Result<()> {
    let paths = AppPaths::load()?;

    if force {
        // force 不走 IPC，直接按 pid 文件发送系统终止信号。
        // 这是 daemon 卡死或 IPC 端口异常时的兜底方案。
        return force_stop(&paths);
    }

    paths.ensure_token()?;
    // 默认 stop 走优雅 shutdown：daemon 收到后会尝试发送 off 并清理 pid。
    match ipc::request(
        &paths,
        "shutdown",
        None,
        None,
        None,
        false,
        Duration::from_secs(2),
    )
    .await
    {
        Ok(response) if response.ok => {
            println!("daemon: stopping");
            Ok(())
        }
        Ok(response) => bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "daemon refused shutdown".to_owned())
        ),
        Err(_) => {
            if let Ok(pid) = paths.read_pid() {
                println!("daemon: unreachable, pid file exists: {pid}");
                println!("hint: run `agent_status_light stop --force` if the daemon is stuck");
            } else {
                println!("daemon: stopped");
            }
            Ok(())
        }
    }
}

async fn start_background() -> Result<()> {
    let paths = AppPaths::load()?;
    paths.ensure_runtime()?;
    paths.ensure_token()?;

    // 先探测已有 daemon，避免重复启动多个进程争抢同一个 BLE 设备。
    if ipc::request(
        &paths,
        "status",
        None,
        None,
        None,
        false,
        Duration::from_millis(300),
    )
    .await
    .is_ok_and(|response| response.ok)
    {
        return Ok(());
    }

    // 后台模式通过重新启动当前可执行文件，并传入 `daemon --foreground` 实现。
    // 子进程 stdout/stderr 写入 daemon.log，父进程无需长期占用终端。
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_file)
        .with_context(|| format!("failed to open log {}", paths.log_file.display()))?;
    let log_for_stderr = log.try_clone().context("failed to clone daemon log file")?;

    let mut command = Command::new(exe);
    command
        .arg("daemon")
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_for_stderr));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    command.spawn().context("failed to spawn daemon")?;

    // 等待 IPC ready。这里总等待时间保持很短，避免 send 自动拉起时拖慢 Hook。
    for _ in 0..20 {
        if ipc::request(
            &paths,
            "status",
            None,
            None,
            None,
            false,
            Duration::from_millis(300),
        )
        .await
        .is_ok_and(|response| response.ok)
        {
            return Ok(());
        }
        time::sleep(Duration::from_millis(100)).await;
    }

    Err(anyhow!(
        "daemon did not become ready; see {}",
        paths.log_file.display()
    ))
}

async fn run_foreground() -> Result<()> {
    let paths = AppPaths::load()?;
    paths.ensure_runtime()?;
    let token = paths.ensure_token()?;
    let (log_tx, log_rx) = log_store::channel();
    let log_task = log_store::spawn_writer(log_rx, paths.clone());

    // 先绑定端口，成功后再写 pid，避免端口被占用时覆盖已有 daemon 的 pid 文件。
    let listener = TcpListener::bind(paths.ipc_addr())
        .await
        .with_context(|| format!("failed to bind IPC at {}", paths.ipc_addr()))?;

    paths.write_pid(std::process::id())?;
    write_ipc_file(&paths)?;
    log_store::emit(
        &log_tx,
        "info",
        "daemon.started",
        format!("pid={}, ipc={}", std::process::id(), paths.ipc_addr()),
    );

    let status: SharedBleStatus = Arc::new(RwLock::new(BleSnapshot::new()));
    let priority_status = Arc::new(RwLock::new(status_priority::PrioritySnapshot::default()));
    let (ble_mode_tx, ble_mode_rx) = mpsc::channel::<String>(32);
    // BLE worker 只接收最终要展示的 mode；优先级合并由 mode router 负责。
    let ble_task = ble::spawn_manager(ble_mode_rx, Arc::clone(&status), log_tx.clone());
    let (mode_tx, mode_rx) = mpsc::channel::<ModeCommand>(64);
    let mode_router_task = spawn_mode_router(
        mode_rx,
        ble_mode_tx,
        Arc::clone(&priority_status),
        log_tx.clone(),
    );
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    info!("AgentStatusLight daemon listening at {}", paths.ipc_addr());

    loop {
        tokio::select! {
            incoming = listener.accept() => {
                // 每个 IPC 连接只处理一条 JSON Lines 请求，处理完立即关闭。
                // 这让协议保持简单，也避免 Hook 客户端需要维护长连接。
                match incoming {
                    Ok((stream, _)) => {
                        let mode_tx = mode_tx.clone();
                        let status = Arc::clone(&status);
                        let priority_status = Arc::clone(&priority_status);
                        let shutdown_tx = shutdown_tx.clone();
                        let ipc_addr = paths.ipc_addr().to_string();
                        let token = token.clone();
                        let log_tx = log_tx.clone();
                        let context = ClientContext {
                            token,
                            mode_tx,
                            status,
                            priority_status,
                            shutdown_tx,
                            ipc_addr,
                            log_tx,
                        };
                        tokio::spawn(async move {
                            if let Err(err) = handle_client(stream, context).await {
                                warn!("failed to handle IPC client: {err:#}");
                            }
                        });
                    }
                    Err(err) => warn!("failed to accept IPC client: {err:#}"),
                }
            }

            _ = shutdown_rx.changed() => {
                // shutdown 由 `agent_status_light stop` 的 IPC 请求触发。
                if *shutdown_rx.borrow() {
                    break;
                }
            }

            result = tokio::signal::ctrl_c() => {
                // 前台调试模式下，Ctrl+C 也走同一套清理流程。
                result.context("failed to listen for Ctrl+C")?;
                break;
            }
        }
    }

    // 退出前尽量投递 off，让灯效回到关闭状态。
    // 这里不强制等待 BLE 写入成功，因为 stop 的首要目标是保证 daemon 可退出。
    let _ = mode_tx.send(ModeCommand::ForceOff).await;
    time::sleep(Duration::from_millis(200)).await;
    mode_router_task.abort();
    ble_task.abort();
    paths.remove_pid()?;
    log_store::emit(&log_tx, "info", "daemon.stopped", "daemon exited");
    drop(log_tx);
    let _ = time::timeout(Duration::from_secs(1), log_task).await;
    info!("AgentStatusLight daemon stopped");
    Ok(())
}

/// 处理单个 IPC 请求。每次连接只读一行 JSON 并返回一行 JSON。
struct ClientContext {
    token: String,
    mode_tx: mpsc::Sender<ModeCommand>,
    status: SharedBleStatus,
    priority_status: Arc<RwLock<status_priority::PrioritySnapshot>>,
    shutdown_tx: watch::Sender<bool>,
    ipc_addr: String,
    log_tx: LogSender,
}

async fn handle_client(stream: TcpStream, context: ClientContext) -> Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    // IPC 协议规定客户端发送一行 JSON。解析失败说明调用方协议不匹配。
    let request = serde_json::from_str::<IpcRequest>(&line)
        .with_context(|| format!("invalid IPC request: {line:?}"))?;
    if request.cmd != "log" {
        log_store::emit(
            &context.log_tx,
            "info",
            "ipc.request",
            format!("cmd={}", request.cmd),
        );
    }

    let response = if request.token != context.token {
        // token 是轻量防护，避免其它本地进程误发灯效指令。
        IpcResponse::error("invalid token")
    } else {
        match request.cmd.as_str() {
            "send" => {
                // daemon 侧再次校验 mode，防止绕过 CLI 直接发 IPC 的无效输入。
                let mode = match request.mode.as_deref().and_then(modes::normalize_mode) {
                    Some(mode) => mode,
                    None => {
                        return write_response(
                            write_half,
                            IpcResponse::error(format!(
                                "invalid mode; valid modes: {}",
                                modes::valid_modes_csv()
                            )),
                        )
                        .await;
                    }
                };

                let source = request
                    .source
                    .as_deref()
                    .map(status_priority::normalize_source)
                    .unwrap_or_else(|| MANUAL_SOURCE.to_owned());
                let ttl = match normalize_ttl_seconds(request.ttl_seconds) {
                    Ok(ttl) => ttl.map(Duration::from_secs),
                    Err(err) => {
                        return write_response(write_half, IpcResponse::error(err.to_string()))
                            .await;
                    }
                };

                if context
                    .mode_tx
                    .send(ModeCommand::Update { source, mode, ttl })
                    .await
                    .is_ok()
                {
                    IpcResponse::ok()
                } else {
                    IpcResponse::error("mode router is not running")
                }
            }
            "status" => {
                // status 读取 BLE worker 维护的快照，不阻塞等待蓝牙操作。
                let snapshot = context.status.read().await.clone();
                let priority_snapshot = context.priority_status.read().await.clone();
                let (effective_mode, sources) = if request.verbose.unwrap_or(false) {
                    (
                        priority_snapshot.effective_mode,
                        Some(priority_snapshot.sources),
                    )
                } else {
                    (None, None)
                };
                IpcResponse {
                    ok: true,
                    error: snapshot.last_error,
                    daemon: Some("running".to_owned()),
                    pid: Some(std::process::id()),
                    ipc: Some(context.ipc_addr),
                    ble: Some(snapshot.state),
                    device: snapshot.device,
                    mode: snapshot.mode,
                    effective_mode,
                    sources,
                }
            }
            "shutdown" => {
                // shutdown 先投递 off，再通知主循环退出。
                let _ = context.mode_tx.send(ModeCommand::ForceOff).await;
                let _ = context.shutdown_tx.send(true);
                IpcResponse::ok()
            }
            "log" => {
                if let (Some(level), Some(event), Some(message)) =
                    (request.level, request.event, request.message)
                {
                    log_store::emit(&context.log_tx, level, event, message);
                    IpcResponse::ok()
                } else {
                    IpcResponse::error("missing log fields")
                }
            }
            other => IpcResponse::error(format!("unknown command {other:?}")),
        }
    };

    write_response(write_half, response).await
}

async fn write_response(
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    response: IpcResponse,
) -> Result<()> {
    let mut payload = serde_json::to_vec(&response)?;
    payload.push(b'\n');
    write_half.write_all(&payload).await?;
    write_half.flush().await?;
    Ok(())
}

#[derive(Debug)]
enum ModeCommand {
    /// 普通 Hook 或 CLI 状态更新。source 用来区分不同 agent。
    Update {
        source: String,
        mode: String,
        ttl: Option<Duration>,
    },
    /// daemon 停止时强制关灯，绕过普通优先级和 source 清理规则。
    ForceOff,
}

fn spawn_mode_router(
    mut rx: mpsc::Receiver<ModeCommand>,
    ble_mode_tx: mpsc::Sender<String>,
    priority_status: Arc<RwLock<status_priority::PrioritySnapshot>>,
    log_tx: LogSender,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut priority = StatusPriority::new();
        let mut interval = time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                command = rx.recv() => {
                    let Some(command) = command else {
                        break;
                    };

                    match command {
                        ModeCommand::Update { source, mode, ttl } => {
                            let now = Instant::now();
                            let effective = priority.apply_update(&source, &mode, ttl, now);
                            update_priority_snapshot(&priority_status, &priority, now).await;
                            log_store::emit(
                                &log_tx,
                                "info",
                                "mode.priority_update",
                                format!(
                                    "source={source}, mode={mode}, priority={}, ttl={}, effective={}",
                                    status_priority::mode_priority(&mode),
                                    ttl.map(|ttl| ttl.as_secs().to_string()).unwrap_or_else(|| "default".to_owned()),
                                    priority.effective_mode().unwrap_or("off")
                                ),
                            );
                            if let Some(mode) = effective {
                                send_effective_mode(&ble_mode_tx, &log_tx, mode).await;
                            }
                        }
                        ModeCommand::ForceOff => {
                            let mode = priority.force_off();
                            update_priority_snapshot(&priority_status, &priority, Instant::now()).await;
                            log_store::emit(&log_tx, "info", "mode.force_off", "forced off");
                            send_effective_mode(&ble_mode_tx, &log_tx, mode).await;
                        }
                    }
                }
                _ = interval.tick() => {
                    let now = Instant::now();
                    if let Some(mode) = priority.expire(now) {
                        update_priority_snapshot(&priority_status, &priority, now).await;
                        log_store::emit(
                            &log_tx,
                            "info",
                            "mode.priority_expired",
                            format!("effective={mode}"),
                        );
                        send_effective_mode(&ble_mode_tx, &log_tx, mode).await;
                    } else {
                        update_priority_snapshot(&priority_status, &priority, now).await;
                    }
                }
            }
        }
    })
}

async fn update_priority_snapshot(
    priority_status: &Arc<RwLock<status_priority::PrioritySnapshot>>,
    priority: &StatusPriority,
    now: Instant,
) {
    *priority_status.write().await = priority.snapshot(now);
}

async fn send_effective_mode(ble_mode_tx: &mpsc::Sender<String>, log_tx: &LogSender, mode: String) {
    log_store::emit(
        log_tx,
        "info",
        "mode.effective_changed",
        format!("mode={mode}"),
    );
    if ble_mode_tx.send(mode).await.is_err() {
        log_store::emit(
            log_tx,
            "error",
            "mode.router_error",
            "BLE worker is not running",
        );
    }
}

fn write_ipc_file(paths: &AppPaths) -> Result<()> {
    // ipc.json 主要用于人工排障。实际通信仍然通过固定的本地 TCP 端口。
    let payload = serde_json::json!({
        "addr": paths.ipc_addr().to_string(),
        "pid": std::process::id(),
    });
    std::fs::write(&paths.ipc_file, serde_json::to_vec_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", paths.ipc_file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_session_id_from_hook_stdin() {
        let session =
            extract_session_from_hook_input(r#"{"session_id":"SESSION-123","cwd":"/tmp/a"}"#);
        assert_eq!(session.as_deref(), Some("session-123"));
    }

    #[test]
    fn falls_back_to_stable_context_hash() {
        let session = extract_session_from_hook_input(r#"{"cwd":"/tmp/project-a"}"#)
            .expect("cwd should produce a fallback session");
        assert!(session.starts_with("hash-"));
    }

    #[test]
    fn extracts_alternate_session_fields() {
        let session = extract_session_from_hook_input(r#"{"tabId":"TAB-9"}"#);
        assert_eq!(session.as_deref(), Some("tab-9"));
    }

    #[test]
    fn fallback_hash_is_stable() {
        assert_eq!(stable_hash("abc"), "e71fa2190541574b");
    }

    #[test]
    fn ttl_validation_rejects_out_of_range_values() {
        assert!(normalize_ttl_seconds(Some(0)).is_err());
        assert!(normalize_ttl_seconds(Some(24 * 60 * 60 + 1)).is_err());
        assert_eq!(normalize_ttl_seconds(Some(60)).unwrap(), Some(60));
    }
}

fn force_stop(paths: &AppPaths) -> Result<()> {
    // force stop 是最后手段，不保证 daemon 有机会发送 off 或断开 BLE。
    let pid = paths.read_pid()?;

    #[cfg(unix)]
    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .context("failed to run kill")?;

    #[cfg(windows)]
    let status = Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/F")
        .status()
        .context("failed to run taskkill")?;

    if !status.success() {
        bail!("failed to force stop daemon pid {pid}");
    }

    paths.remove_pid()?;
    println!("daemon: force stopped pid {pid}");
    Ok(())
}
