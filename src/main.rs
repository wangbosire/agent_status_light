//! AgentStatusLight 电脑端入口。
//!
//! 入口层只负责解析命令并分发，具体实现放在各业务模块中。

mod ble;
mod cli;
mod config;
mod daemon;
mod hooks;
mod install;
mod ipc;
mod log_store;
mod modes;
mod status_priority;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent_status_light=info,warn".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { foreground } => daemon::run(foreground).await,
        Commands::Send {
            mode,
            source,
            session,
            ttl,
            quiet,
            hook_id: _,
            strict,
        } => daemon::send_mode(&mode, &source, &session, ttl, quiet, strict).await,
        Commands::Status { verbose } => daemon::print_status(verbose).await,
        Commands::Logs { limit } => log_store::print_recent(limit),
        Commands::Stop { force } => daemon::stop(force).await,
        Commands::Install { target, dir } => install::install(target, dir.as_deref()).await,
        Commands::Uninstall { target, dir } => install::uninstall(target, dir.as_deref()).await,
    }
}
