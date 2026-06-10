//! Hook 安装和卸载。
//!
//! `--dir` 存在时把 Hook 写入项目级 agent 配置；不传时写入用户全局 agent 配置。
//! Hook 配置引用固定 `.agent-status-light/bin` 下的可执行文件副本，避免用户删除解压目录后失效。

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{cli::HookTarget, config, hooks, ipc};

#[derive(Debug, Deserialize, Serialize)]
struct InstallConfig {
    /// 安装清单版本，后续字段调整时用于兼容旧清单。
    version: u8,
    /// 当前安装的 Hook 目标，例如 codex/cursor/claude。
    target: String,
    /// 安装根目录，便于用户排查当前产物位置。
    install_root: String,
    /// 复制后的可执行文件路径，Hook 配置会引用它。
    binary: String,
    /// 真实写入的 Codex/Cursor/Claude 配置文件路径。
    hook_config: String,
    /// install 前该配置文件是否已经存在。卸载时用它判断能不能删除空文件。
    created_hook_config: bool,
}

pub async fn install(target: HookTarget, dir: Option<&Path>) -> Result<()> {
    // root 存放 AgentStatusLight 自己的辅助产物；真实 Hook 写到 target_config_path。
    let root = config::install_root(dir)?;
    let hook_config_path = hooks::target_config_path(target, dir)?;
    let app_paths = config::AppPaths::load()?;
    let _ = ipc::log_event(
        &app_paths,
        "info",
        "install.started",
        format!(
            "target={}, root={}, hook_config={}",
            target.as_str(),
            root.display(),
            hook_config_path.display()
        ),
    )
    .await;

    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;

    // 复制当前可执行文件到固定目录，避免用户删除解压包后 Hook 路径失效。
    let binary = copy_current_binary(&bin_dir)?;
    let binary_for_hook = path_for_hook(&binary);
    let manifest_path = manifest_path(&root, target);
    let created_hook_config = read_manifest(&manifest_path)?
        .map(|manifest| manifest.created_hook_config)
        .unwrap_or_else(|| !hook_config_path.exists());
    let mut hook_config = hooks::read_json(&hook_config_path)?;
    let fragment = hooks::render_config(target, &binary_for_hook);
    hooks::merge_config(&mut hook_config, fragment)?;
    write_json(&hook_config_path, &hook_config)?;

    let config: InstallConfig = InstallConfig {
        version: 1,
        target: target.as_str().to_owned(),
        install_root: root.display().to_string(),
        binary: binary.display().to_string(),
        hook_config: hook_config_path.display().to_string(),
        created_hook_config,
    };
    fs::write(&manifest_path, serde_json::to_vec_pretty(&config)?)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    println!(
        "installed {} hook config to {}",
        target.as_str(),
        hook_config_path.display()
    );
    println!("binary: {}", binary.display());
    println!("manifest: {}", manifest_path.display());
    let _ = ipc::log_event(
        &app_paths,
        "info",
        "install.finished",
        format!(
            "target={}, hook_config={}",
            target.as_str(),
            hook_config_path.display()
        ),
    )
    .await;
    Ok(())
}

pub async fn uninstall(target: HookTarget, dir: Option<&Path>) -> Result<()> {
    // uninstall 只移除 AgentStatusLight 自己安装的 Hook 条目，保留用户原有配置。
    let root = config::install_root(dir)?;
    let hook_config_path = hooks::target_config_path(target, dir)?;
    let manifest_path = manifest_path(&root, target);
    let manifest = read_manifest(&manifest_path)?;
    let app_paths = config::AppPaths::load()?;
    let _ = ipc::log_event(
        &app_paths,
        "info",
        "uninstall.started",
        format!(
            "target={}, root={}, hook_config={}",
            target.as_str(),
            root.display(),
            hook_config_path.display()
        ),
    )
    .await;

    if hook_config_path.exists() {
        let mut hook_config = hooks::read_json(&hook_config_path)?;
        let removed = hooks::remove_installed_entries(&mut hook_config);

        if removed == 0 {
            println!(
                "no AgentStatusLight hooks found in {}",
                hook_config_path.display()
            );
        } else if manifest
            .as_ref()
            .is_some_and(|manifest| manifest.created_hook_config)
            && !hooks::has_any_hook_entries(&hook_config)
        {
            fs::remove_file(&hook_config_path)
                .with_context(|| format!("failed to remove {}", hook_config_path.display()))?;
            println!("removed empty hook config {}", hook_config_path.display());
        } else {
            write_json(&hook_config_path, &hook_config)?;
            println!(
                "removed {removed} AgentStatusLight hook entries from {}",
                hook_config_path.display()
            );
        }
    } else {
        println!("no hook config found at {}", hook_config_path.display());
    }

    if manifest_path.exists() {
        fs::remove_file(&manifest_path)
            .with_context(|| format!("failed to remove {}", manifest_path.display()))?;
    }

    let _ = ipc::log_event(
        &app_paths,
        "info",
        "uninstall.finished",
        format!(
            "target={}, hook_config={}",
            target.as_str(),
            hook_config_path.display()
        ),
    )
    .await;
    Ok(())
}

fn copy_current_binary(bin_dir: &Path) -> Result<PathBuf> {
    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    let file_name = if cfg!(windows) {
        "agent_status_light.exe"
    } else {
        "agent_status_light"
    };
    let dest = bin_dir.join(file_name);
    fs::copy(&current_exe, &dest).with_context(|| {
        format!(
            "failed to copy {} to {}",
            current_exe.display(),
            dest.display()
        )
    })?;
    Ok(dest)
}

fn path_for_hook(path: &Path) -> String {
    // 先使用 display 字符串。后续如果要生成 shell 脚本，可在这里集中处理转义。
    path.display().to_string()
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    fs::write(path, hooks::to_pretty_json(value)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn manifest_path(root: &Path, target: HookTarget) -> PathBuf {
    root.join(format!("config.{}.json", target.as_str()))
}

fn read_manifest(path: &Path) -> Result<Option<InstallConfig>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(manifest))
}
