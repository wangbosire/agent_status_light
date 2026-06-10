//! 灯效模式定义和校验。
//!
//! CLI 和 IPC 都统一走这里，避免不同入口接受的 mode 不一致。

pub const VALID_MODES: &[&str] = &[
    "demo", "thinking", "ai", "busy", "success", "error", "alarm", "traffic", "off", "red",
    "yellow", "green",
];

pub fn normalize_mode(mode: &str) -> Option<String> {
    let mode = mode.trim().to_ascii_lowercase();
    VALID_MODES
        .iter()
        .any(|candidate| *candidate == mode)
        .then_some(mode)
}

pub fn valid_modes_csv() -> String {
    VALID_MODES.join(", ")
}
