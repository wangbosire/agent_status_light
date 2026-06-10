//! 路径和运行时文件管理。
//!
//! AgentStatusLight 是一个单文件命令。runtime 和安装清单统一放在固定的
//! `.agent-status-light` 目录中，避免把另一份可执行文件复制到系统配置目录。

use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use uuid::Uuid;

pub const IPC_PORT: u16 = 47_631;

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// daemon 运行时目录，保存 pid、日志、IPC 信息和 token。
    pub runtime_dir: PathBuf,
    /// daemon pid 文件，用于 status 和 stop --force。
    pub pid_file: PathBuf,
    /// daemon stdout/stderr 日志，主要用于排查后台进程崩溃。
    pub log_file: PathBuf,
    /// 结构化事件日志，最多保留最近 1000 条。
    pub events_file: PathBuf,
    /// IPC 元信息文件，方便人工查看当前 daemon 端口和 pid。
    pub ipc_file: PathBuf,
    /// 本机共享 token 文件，IPC client/server 都读取它。
    pub token_file: PathBuf,
}

impl AppPaths {
    /// 解析 AgentStatusLight 状态目录和 runtime 文件路径。
    pub fn load() -> Result<Self> {
        let config_dir = default_app_root()?;
        let runtime_dir = config_dir.join("runtime");

        Ok(Self {
            pid_file: runtime_dir.join("daemon.pid"),
            log_file: runtime_dir.join("daemon.log"),
            events_file: runtime_dir.join("events.jsonl"),
            ipc_file: runtime_dir.join("ipc.json"),
            token_file: runtime_dir.join("token"),
            runtime_dir,
        })
    }

    /// daemon 固定监听本机端口，避免暴露到局域网。
    pub fn ipc_addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), IPC_PORT)
    }

    /// 创建 runtime 目录，daemon、日志和 token 都依赖它。
    pub fn ensure_runtime(&self) -> Result<()> {
        fs::create_dir_all(&self.runtime_dir).with_context(|| {
            format!(
                "failed to create runtime directory {}",
                self.runtime_dir.display()
            )
        })?;
        Ok(())
    }

    /// 读取或创建本机 IPC token。
    pub fn ensure_token(&self) -> Result<String> {
        self.ensure_runtime()?;
        if self.token_file.exists() {
            return self.read_token();
        }

        let token = Uuid::new_v4().to_string();
        fs::write(&self.token_file, &token)
            .with_context(|| format!("failed to write token {}", self.token_file.display()))?;
        Ok(token)
    }

    /// 读取已有 token。daemon 和 client 必须读取到同一个 token。
    pub fn read_token(&self) -> Result<String> {
        let token = fs::read_to_string(&self.token_file)
            .with_context(|| format!("failed to read token {}", self.token_file.display()))?;
        Ok(token.trim().to_owned())
    }

    /// 写入 daemon pid，供 status/stop --force 使用。
    pub fn write_pid(&self, pid: u32) -> Result<()> {
        self.ensure_runtime()?;
        fs::write(&self.pid_file, pid.to_string())
            .with_context(|| format!("failed to write pid {}", self.pid_file.display()))?;
        Ok(())
    }

    /// 读取 daemon pid。
    pub fn read_pid(&self) -> Result<u32> {
        let pid = fs::read_to_string(&self.pid_file)
            .with_context(|| format!("failed to read pid {}", self.pid_file.display()))?;
        pid.trim()
            .parse::<u32>()
            .with_context(|| format!("invalid pid in {}", self.pid_file.display()))
    }

    /// 清理 daemon pid 文件。
    pub fn remove_pid(&self) -> Result<()> {
        if self.pid_file.exists() {
            fs::remove_file(&self.pid_file)
                .with_context(|| format!("failed to remove pid {}", self.pid_file.display()))?;
        }
        Ok(())
    }
}

/// 计算 install/uninstall 的目标目录。
pub fn install_root(dir: Option<&Path>) -> Result<PathBuf> {
    // `dir` 只影响目标 Agent 的 Hook 配置位置；AgentStatusLight 自己的文件始终放在固定目录。
    let _ = dir;
    default_app_root()
}

#[cfg(windows)]
fn default_app_root() -> Result<PathBuf> {
    Ok(PathBuf::from(r"C:\.agent-status-light"))
}

#[cfg(not(windows))]
fn default_app_root() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(PathBuf::from(home).join(".agent-status-light"))
}
