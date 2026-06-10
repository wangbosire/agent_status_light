//! 多 Agent 状态优先级路由。
//!
//! Hook 可能来自 Codex、Cursor、Claude 等多个 agent。如果简单采用“最后一次 send 覆盖”，
//! 低优先级的 `success/off` 很容易盖掉另一个 agent 正在执行的 `busy/alarm`。
//! 这个模块按 source 保存每个 agent 的最新状态，并计算当前真正应该展示的最高优先级 mode。

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

/// 手动 CLI 请求的默认来源。
///
/// 手动执行 `send --mode off` 时通常代表“我想把灯关掉”，因此 daemon 会把它当成全局清空。
pub const MANUAL_SOURCE: &str = "manual";
pub const MANUAL_SESSION: &str = "manual";
pub const DEFAULT_SESSION: &str = "default";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PrioritySnapshot {
    /// 当前最终展示的 mode。
    pub effective_mode: Option<String>,
    /// 当前仍然活跃的 source/session 状态。
    pub sources: Vec<PrioritySourceStatus>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrioritySourceStatus {
    /// source/session 唯一键，例如 `cursor:session-123`。
    pub source: String,
    pub mode: String,
    pub priority: u8,
    pub expires_in_secs: u64,
}

#[derive(Debug, Clone)]
struct ActiveStatus {
    mode: String,
    updated_at: Instant,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct StatusPriority {
    active: HashMap<String, ActiveStatus>,
    effective_mode: Option<String>,
}

impl StatusPriority {
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用一条普通状态更新，返回需要写入 BLE 的新 mode。
    pub fn apply_update(
        &mut self,
        source: &str,
        mode: &str,
        ttl: Option<Duration>,
        now: Instant,
    ) -> Option<String> {
        self.prune_expired(now);
        let source = normalize_source(source);

        if mode == "off" {
            // 手动 off 用来全局关灯；Hook off 只清除对应 agent 的状态，避免误关其它 agent。
            if source == MANUAL_SOURCE {
                self.active.clear();
            } else {
                self.remove_source_entries(&source);
            }
        } else {
            if is_source_root_key(&source) {
                self.remove_source_entries(&source);
            }
            self.active.insert(
                source,
                ActiveStatus {
                    mode: mode.to_owned(),
                    updated_at: now,
                    expires_at: now + ttl.unwrap_or_else(|| mode_ttl(mode)),
                },
            );
        }

        self.refresh_effective(false)
    }

    /// 强制关闭灯效。daemon shutdown 时使用它绕过优先级判断。
    pub fn force_off(&mut self) -> String {
        self.active.clear();
        self.effective_mode = Some("off".to_owned());
        "off".to_owned()
    }

    /// 清理过期状态，返回过期后需要写入 BLE 的新 mode。
    pub fn expire(&mut self, now: Instant) -> Option<String> {
        self.prune_expired(now);
        if self.active.is_empty() && self.effective_mode.is_none() {
            return None;
        }
        self.refresh_effective(false)
    }

    pub fn effective_mode(&self) -> Option<&str> {
        self.effective_mode.as_deref()
    }

    pub fn snapshot(&self, now: Instant) -> PrioritySnapshot {
        let mut sources = self
            .active
            .iter()
            .map(|(source, status)| PrioritySourceStatus {
                source: source.clone(),
                mode: status.mode.clone(),
                priority: mode_priority(&status.mode),
                expires_in_secs: status.expires_at.saturating_duration_since(now).as_secs(),
            })
            .collect::<Vec<_>>();

        sources.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.source.cmp(&right.source))
        });

        PrioritySnapshot {
            effective_mode: self.effective_mode.clone(),
            sources,
        }
    }

    fn prune_expired(&mut self, now: Instant) {
        self.active
            .retain(|_, status| status.expires_at.saturating_duration_since(now) > Duration::ZERO);
    }

    fn refresh_effective(&mut self, force: bool) -> Option<String> {
        let next = self.best_mode().unwrap_or("off").to_owned();
        if !force && self.effective_mode.as_deref() == Some(next.as_str()) {
            return None;
        }

        self.effective_mode = Some(next.clone());
        Some(next)
    }

    fn best_mode(&self) -> Option<&str> {
        self.active
            .values()
            .max_by(|left, right| {
                mode_priority(&left.mode)
                    .cmp(&mode_priority(&right.mode))
                    .then_with(|| left.updated_at.cmp(&right.updated_at))
            })
            .map(|status| status.mode.as_str())
    }

    fn remove_source_entries(&mut self, source: &str) {
        if is_source_root_key(source) {
            let session_prefix = format!("{source}:");
            self.active
                .retain(|key, _| key != source && !key.starts_with(&session_prefix));
        } else {
            self.active.remove(source);
        }
    }
}

pub fn normalize_source(source: &str) -> String {
    let source = source.trim();
    if source.is_empty() {
        MANUAL_SOURCE.to_owned()
    } else {
        source.to_ascii_lowercase()
    }
}

pub fn normalize_session(session: &str) -> String {
    let session = session.trim();
    if session.is_empty() {
        DEFAULT_SESSION.to_owned()
    } else {
        session.to_ascii_lowercase()
    }
}

pub fn source_key(source: &str, session: &str) -> String {
    let source = normalize_source(source);
    let session = normalize_session(session);

    // 保留手动默认 key，确保 `send --mode off` 仍能触发“全局关灯”的语义。
    if source == MANUAL_SOURCE && session == MANUAL_SESSION {
        MANUAL_SOURCE.to_owned()
    } else {
        format!("{source}:{session}")
    }
}

fn is_source_root_key(source: &str) -> bool {
    !source.contains(':')
}

/// mode 优先级。数值越大越应该优先展示。
///
/// 设计意图：
/// - `alarm/error` 代表异常或严重阻塞，必须压过普通运行状态。
/// - `busy/ai` 代表 agent 正在执行工具，应压过“等待用户”的 yellow。
/// - `yellow/thinking` 代表等待或思考，压过短暂的 success。
/// - `success/green/demo/traffic` 是收尾或展示状态，优先级较低。
/// - `off` 不进入活跃状态池，只用于清除 source 或全局关灯。
pub fn mode_priority(mode: &str) -> u8 {
    match mode {
        "alarm" => 100,
        "error" => 90,
        "busy" => 70,
        "ai" => 60,
        "yellow" => 55,
        "thinking" => 50,
        "success" => 40,
        "red" => 35,
        "green" => 30,
        "demo" => 20,
        "traffic" => 10,
        "off" => 0,
        _ => 0,
    }
}

/// 每类状态在 daemon 内的保留时间。
///
/// 固件本身也有超时保护，但 daemon 需要更早知道“高优先级状态已经过期”，
/// 这样才能自动回落到其它 agent 仍然活跃的状态。
pub fn mode_ttl(mode: &str) -> Duration {
    match mode {
        "alarm" | "yellow" => Duration::from_secs(10 * 60),
        "error" => Duration::from_secs(90),
        "busy" | "ai" | "thinking" => Duration::from_secs(5 * 60),
        "success" => Duration::from_secs(20),
        "red" | "green" | "demo" | "traffic" => Duration::from_secs(60),
        _ => Duration::from_secs(30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_priority_source_wins_until_it_expires() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("cursor", "thinking", None, start),
            Some("thinking".to_owned())
        );
        assert_eq!(
            priority.apply_update("codex", "error", None, start),
            Some("error".to_owned())
        );
        assert_eq!(
            priority.apply_update("claude", "success", None, start),
            None
        );

        let after_error_expires = start + mode_ttl("error") + Duration::from_secs(1);
        assert_eq!(
            priority.expire(after_error_expires),
            Some("thinking".to_owned())
        );
    }

    #[test]
    fn working_status_beats_waiting_user_status() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("claude", "yellow", None, start),
            Some("yellow".to_owned())
        );
        assert_eq!(
            priority.apply_update("cursor", "busy", None, start),
            Some("busy".to_owned())
        );
        assert_eq!(priority.apply_update("codex", "success", None, start), None);
    }

    #[test]
    fn source_off_only_clears_that_source() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("codex", "busy", None, start),
            Some("busy".to_owned())
        );
        assert_eq!(
            priority.apply_update("cursor", "alarm", None, start),
            Some("alarm".to_owned())
        );
        assert_eq!(
            priority.apply_update("cursor", "off", None, start),
            Some("busy".to_owned())
        );
    }

    #[test]
    fn manual_off_clears_all_sources() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("codex", "busy", None, start),
            Some("busy".to_owned())
        );
        assert_eq!(
            priority.apply_update(MANUAL_SOURCE, "off", None, start),
            Some("off".to_owned())
        );
        assert_eq!(priority.effective_mode(), Some("off"));
    }

    #[test]
    fn same_agent_different_sessions_do_not_overwrite_each_other() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("cursor:session-a", "busy", None, start),
            Some("busy".to_owned())
        );
        assert_eq!(
            priority.apply_update("cursor:session-b", "alarm", None, start),
            Some("alarm".to_owned())
        );
        assert_eq!(
            priority.apply_update("cursor:session-b", "off", None, start),
            Some("busy".to_owned())
        );
    }

    #[test]
    fn source_root_update_clears_old_sessions_for_that_source() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("claude:session-a", "yellow", None, start),
            Some("yellow".to_owned())
        );
        assert_eq!(
            priority.apply_update("claude", "success", None, start),
            Some("success".to_owned())
        );

        let snapshot = priority.snapshot(start);
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].source, "claude");
        assert_eq!(snapshot.sources[0].mode, "success");
    }

    #[test]
    fn source_root_off_clears_all_sessions_for_that_source() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        priority.apply_update("claude:session-a", "yellow", None, start);
        priority.apply_update("claude:session-b", "busy", None, start);
        priority.apply_update("cursor:session-a", "busy", None, start);

        assert_eq!(priority.apply_update("claude", "off", None, start), None);

        let snapshot = priority.snapshot(start);
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].source, "cursor:session-a");
    }

    #[test]
    fn source_key_keeps_manual_off_global() {
        assert_eq!(source_key(MANUAL_SOURCE, MANUAL_SESSION), MANUAL_SOURCE);
        assert_eq!(source_key("cursor", "abc"), "cursor:abc");
    }

    #[test]
    fn custom_ttl_controls_expiration() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        assert_eq!(
            priority.apply_update("codex:short", "busy", Some(Duration::from_secs(2)), start),
            Some("busy".to_owned())
        );
        assert_eq!(priority.expire(start + Duration::from_secs(1)), None);
        assert_eq!(
            priority.expire(start + Duration::from_secs(3)),
            Some("off".to_owned())
        );
    }

    #[test]
    fn snapshot_lists_sources_by_priority() {
        let start = Instant::now();
        let mut priority = StatusPriority::new();

        priority.apply_update("cursor:a", "success", None, start);
        priority.apply_update("cursor:b", "busy", None, start);

        let snapshot = priority.snapshot(start);
        assert_eq!(snapshot.effective_mode.as_deref(), Some("busy"));
        assert_eq!(snapshot.sources[0].source, "cursor:b");
        assert_eq!(snapshot.sources[0].mode, "busy");
    }
}
