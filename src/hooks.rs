//! Codex / Cursor / Claude Hook 配置生成与合并。
//!
//! 这里不再生成“推荐模板”，而是按各工具真实配置文件 schema 生成可直接使用的 JSON。
//! 安装时只追加 AgentStatusLight 自己的 Hook 条目；卸载时也只移除命令中包含本工具
//! `send --mode` 的条目，尽量不影响用户已有配置。

use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Map, Value, json};

use crate::cli::HookTarget;

/// 用于识别本工具安装的 Hook 命令。卸载时会结合 `send --mode` 一起判断，避免误删。
const COMMAND_MARKER: &str = "agent_status_light";
/// Cursor hooks.json 当前官方 schema 使用的版本号。
const CURSOR_HOOK_VERSION: u64 = 1;

/// 计算真实 Hook 配置文件路径。
///
/// - 传入 `--dir`：安装到项目级配置目录。
/// - 未传 `--dir`：安装到用户家目录下的全局配置目录。
pub fn target_config_path(target: HookTarget, dir: Option<&Path>) -> Result<PathBuf> {
    let base = match dir {
        Some(dir) => dir.to_path_buf(),
        None => home_dir()?,
    };

    let path = match target {
        HookTarget::Codex => base.join(".codex").join("hooks.json"),
        HookTarget::Cursor => base.join(".cursor").join("hooks.json"),
        HookTarget::Claude => base.join(".claude").join("settings.json"),
    };

    Ok(path)
}

fn home_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        return env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("failed to resolve user home directory"));
    }

    #[cfg(not(windows))]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("failed to resolve user home directory"))
    }
}

/// 生成目标工具可直接写入配置文件的 Hook 片段。
///
/// 返回值是一个完整 JSON 对象，但只包含 AgentStatusLight 需要插入的字段。
/// install 阶段会把这个对象合并进已有配置。
pub fn render_config(target: HookTarget, binary: &str) -> Value {
    match target {
        HookTarget::Codex => codex_config(binary),
        HookTarget::Cursor => cursor_config(binary),
        HookTarget::Claude => claude_config(binary),
    }
}

/// 把 AgentStatusLight Hook 片段合并进已有 JSON。
///
/// 合并前会先删除旧的 AgentStatusLight 条目，因此重复执行 install 是幂等的。
pub fn merge_config(existing: &mut Value, fragment: Value) -> Result<()> {
    ensure_object(existing)?;
    remove_installed_entries(existing);

    let existing_object = existing
        .as_object_mut()
        .expect("ensure_object guarantees an object");
    let fragment_object = fragment
        .as_object()
        .ok_or_else(|| anyhow!("generated hook config is not an object"))?;

    // Cursor 的 `version` 是顶层字段；如果用户已有配置则保留用户版本。
    for (key, value) in fragment_object {
        if key != "hooks" {
            existing_object
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    let target_hooks = ensure_hooks_object(existing_object)?;
    let source_hooks = fragment_object
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("generated hook config has no hooks object"))?;

    for (event, entries) in source_hooks {
        let entries = entries
            .as_array()
            .ok_or_else(|| anyhow!("generated hook entries for {event} are not an array"))?;
        let target_entries = target_hooks
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let target_entries = target_entries
            .as_array_mut()
            .ok_or_else(|| anyhow!("existing hooks.{event} is not an array"))?;
        target_entries.extend(entries.iter().cloned());
    }

    Ok(())
}

/// 从配置 JSON 中移除本工具安装的 Hook 条目，返回删除数量。
pub fn remove_installed_entries(config: &mut Value) -> usize {
    prune_agent_entries(config)
}

/// 判断配置是否已经不包含任何 Hook 条目。
///
/// 这个函数只用于决定是否可以删除由本工具创建的空配置文件。
pub fn has_any_hook_entries(config: &Value) -> bool {
    config
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| {
            hooks.values().any(|entries| {
                entries
                    .as_array()
                    .is_some_and(|entries| !entries.is_empty())
            })
        })
}

/// Codex 使用独立的 `hooks.json`，事件名是 PascalCase，事件项内再包含 `hooks` 数组。
fn codex_config(binary: &str) -> Value {
    json!({
        "hooks": {
            "SessionStart": [
                hook_group(
                    Some("startup|resume"),
                    vec![codex_command(binary, "green", "codex", "AgentStatusLight: online")]
                )
            ],
            "UserPromptSubmit": [
                hook_group(
                    None,
                    vec![codex_command(binary, "thinking", "codex", "AgentStatusLight: thinking")]
                )
            ],
            "PreToolUse": [
                hook_group(
                    Some("Bash"),
                    vec![codex_command(binary, "busy", "codex", "AgentStatusLight: command running")]
                ),
                hook_group(
                    Some("apply_patch"),
                    vec![codex_command(binary, "ai", "codex", "AgentStatusLight: editing")]
                )
            ],
            "PermissionRequest": [
                hook_group(
                    None,
                    vec![codex_command(binary, "yellow", "codex", "AgentStatusLight: approval required")]
                )
            ],
            "PostToolUse": [
                hook_group(
                    None,
                    vec![codex_command(binary, "success", "codex", "AgentStatusLight: tool finished")]
                )
            ],
            "Stop": [
                hook_group(None, vec![plain_command(binary, "success", "codex")])
            ]
        }
    })
}

/// Cursor 使用 `version: 1` 加小写事件名，事件项直接是 command 对象。
fn cursor_config(binary: &str) -> Value {
    json!({
        "version": CURSOR_HOOK_VERSION,
        "hooks": {
            "sessionStart": [
                cursor_command(binary, "green", "cursor")
            ],
            "beforeSubmitPrompt": [
                cursor_command(binary, "thinking", "cursor")
            ],
            "afterAgentResponse": [
                cursor_matched_command(binary, "success", "cursor", "AgentResponse")
            ],
            "preToolUse": [
                cursor_matched_command(binary, "yellow", "cursor", "AskQuestion"),
                cursor_matched_command(binary, "ai", "cursor", "Write|Edit|MultiEdit")
            ],
            "beforeShellExecution": [
                cursor_matched_command(binary, "busy", "cursor", ".*")
            ],
            "afterShellExecution": [
                cursor_matched_command(binary, "success", "cursor", ".*")
            ],
            "stop": [
                cursor_stop_command(binary, "success", "cursor")
            ],
            "sessionEnd": [
                cursor_command(binary, "green", "cursor")
            ]
        }
    })
}

/// Claude Code 的 hooks 存放在 settings.json 中，结构与 Codex 类似。
fn claude_config(binary: &str) -> Value {
    json!({
        "hooks": {
            "SessionStart": [
                hook_group(Some("startup|resume"), vec![plain_command(binary, "green", "claude")])
            ],
            "UserPromptSubmit": [
                hook_group(None, vec![plain_command(binary, "thinking", "claude")])
            ],
            "PreToolUse": [
                hook_group(Some("Bash"), vec![plain_command(binary, "busy", "claude")]),
                hook_group(Some("Write|Edit|MultiEdit"), vec![plain_command(binary, "ai", "claude")])
            ],
            "PermissionRequest": [
                hook_group(None, vec![plain_command(binary, "yellow", "claude")])
            ],
            "PermissionDenied": [
                hook_group(None, vec![plain_command(binary, "error", "claude")])
            ],
            "Elicitation": [
                hook_group(None, vec![plain_command(binary, "yellow", "claude")])
            ],
            "ElicitationResult": [
                hook_group(None, vec![plain_command(binary, "thinking", "claude")])
            ],
            "PostToolUse": [
                hook_group(None, vec![plain_command(binary, "success", "claude")])
            ],
            "PostToolUseFailure": [
                hook_group(None, vec![plain_command(binary, "error", "claude")])
            ],
            "Notification": [
                hook_group(Some("permission_prompt|elicitation_dialog"), vec![plain_command(binary, "yellow", "claude")])
            ],
            "Stop": [
                hook_group(None, vec![plain_command(binary, "success", "claude")])
            ],
            "StopFailure": [
                hook_group(None, vec![plain_command(binary, "error", "claude")])
            ],
            "SubagentStop": [
                hook_group(None, vec![plain_command(binary, "thinking", "claude")])
            ]
        }
    })
}

fn hook_group(matcher: Option<&str>, hooks: Vec<Value>) -> Value {
    let mut group = Map::new();
    if let Some(matcher) = matcher {
        group.insert("matcher".to_owned(), Value::String(matcher.to_owned()));
    }
    group.insert("hooks".to_owned(), Value::Array(hooks));
    Value::Object(group)
}

fn codex_command(binary: &str, mode: &str, source: &str, status_message: &str) -> Value {
    json!({
        "type": "command",
        "command": command(binary, mode, source),
        "statusMessage": status_message,
        "timeout": 5
    })
}

fn plain_command(binary: &str, mode: &str, source: &str) -> Value {
    json!({
        "type": "command",
        "command": command(binary, mode, source),
        "timeout": 5
    })
}

fn cursor_command(binary: &str, mode: &str, source: &str) -> Value {
    json!({
        "command": command(binary, mode, source),
        "timeout": 5,
        "failClosed": false
    })
}

fn cursor_matched_command(binary: &str, mode: &str, source: &str, matcher: &str) -> Value {
    json!({
        "command": command(binary, mode, source),
        "timeout": 5,
        "matcher": matcher,
        "failClosed": false
    })
}

fn cursor_stop_command(binary: &str, mode: &str, source: &str) -> Value {
    json!({
        "command": command(binary, mode, source),
        "timeout": 5,
        "loop_limit": 1,
        "failClosed": false
    })
}

fn command(binary: &str, mode: &str, source: &str) -> String {
    format!(
        "{} send --mode {mode} --source {} --session auto --ttl {} --quiet --hook-id agent-status-light",
        binary,
        source,
        hook_ttl(mode)
    )
}

fn hook_ttl(mode: &str) -> u64 {
    match mode {
        "alarm" | "yellow" => 30 * 60,
        "busy" | "ai" | "thinking" => 30 * 60,
        "error" => 5 * 60,
        "success" => 30,
        "red" | "green" | "demo" | "traffic" => 2 * 60,
        _ => 60,
    }
}

fn ensure_object(value: &mut Value) -> Result<()> {
    if value.is_null() {
        *value = Value::Object(Map::new());
    }

    value
        .as_object()
        .map(|_| ())
        .ok_or_else(|| anyhow!("existing hook config root is not a JSON object"))
}

fn ensure_hooks_object(
    existing_object: &mut Map<String, Value>,
) -> Result<&mut Map<String, Value>> {
    let hooks = existing_object
        .entry("hooks".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("existing hooks field is not a JSON object"))
}

fn prune_agent_entries(value: &mut Value) -> usize {
    match value {
        Value::Array(entries) => {
            let mut removed = 0;
            let old_entries = std::mem::take(entries);
            for mut entry in old_entries {
                if contains_agent_command(&entry) {
                    removed += 1;
                } else {
                    removed += prune_agent_entries(&mut entry);
                    entries.push(entry);
                }
            }
            removed
        }
        Value::Object(object) => {
            let removed = object.values_mut().map(prune_agent_entries).sum();
            remove_empty_hook_arrays(object);
            removed
        }
        _ => 0,
    }
}

fn remove_empty_hook_arrays(object: &mut Map<String, Value>) {
    let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };

    hooks.retain(|_, entries| !entries.as_array().is_some_and(|entries| entries.is_empty()));
}

fn contains_agent_command(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            if key == "command" {
                value.as_str().is_some_and(is_agent_command)
            } else {
                contains_agent_command(value)
            }
        }),
        Value::Array(entries) => entries.iter().any(contains_agent_command),
        _ => false,
    }
}

fn is_agent_command(command: &str) -> bool {
    command.contains("--hook-id agent-status-light")
        || (command.contains(COMMAND_MARKER)
            && command.contains("send --mode")
            && (command.contains("--source 'codex'")
                || command.contains("--source \"codex\"")
                || command.contains("--source codex")
                || command.contains("--source 'cursor'")
                || command.contains("--source \"cursor\"")
                || command.contains("--source cursor")
                || command.contains("--source 'claude'")
                || command.contains("--source \"claude\"")
                || command.contains("--source claude")))
}

pub fn read_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn to_pretty_json(value: &Value) -> Result<Vec<u8>> {
    let mut output = serde_json::to_vec_pretty(value)?;
    output.push(b'\n');
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_hooks_cover_prompt_and_permission_approval() {
        let config = render_config(HookTarget::Codex, "/tmp/agent_status_light");

        assert_event_command_contains(&config, "UserPromptSubmit", "--mode thinking");
        assert_event_command_contains(&config, "PermissionRequest", "--mode yellow");
        assert_event_command_contains(&config, "PostToolUse", "--mode success");
    }

    #[test]
    fn claude_hooks_cover_permission_approval() {
        let config = render_config(HookTarget::Claude, "/tmp/agent_status_light");

        assert_event_command_contains(&config, "PermissionRequest", "--mode yellow");
        assert_event_command_contains(&config, "Notification", "--mode yellow");
    }

    #[test]
    fn hook_commands_are_not_shell_quoted() {
        let hook_command = command(
            "/tmp/agent-status-light/bin/agent_status_light",
            "alarm",
            "claude",
        );

        assert_eq!(
            hook_command,
            "/tmp/agent-status-light/bin/agent_status_light send --mode alarm --source claude --session auto --ttl 1800 --quiet --hook-id agent-status-light"
        );
    }

    fn assert_event_command_contains(config: &Value, event: &str, expected: &str) {
        let event_config = config
            .get("hooks")
            .and_then(|hooks| hooks.get(event))
            .unwrap_or_else(|| panic!("missing {event} hook"));
        assert!(
            event_config.to_string().contains(expected),
            "{event} hook should contain {expected}: {event_config}"
        );
    }
}
