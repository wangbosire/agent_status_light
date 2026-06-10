//! 命令行参数定义。
//!
//! 这个模块只描述用户能输入哪些命令，具体业务逻辑放在 daemon/install 等模块中。

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "agent_status_light")]
#[command(about = "通过 BLE 控制 ESP32-C3 AgentStatusLight")]
#[command(
    long_about = "AgentStatusLight 是一个电脑端命令行工具，用来控制已经烧录好的 ESP32-C3 状态灯。\n\n它会在电脑上启动一个后台 daemon，由 daemon 长期维护 BLE 蓝牙连接。Codex、Cursor、Claude 等 Agent 的 Hook 只需要调用 send 命令，把状态通过本地 IPC 发给 daemon，避免每次 Hook 触发都重新扫描和连接蓝牙。",
    after_help = "常用流程：\n  1. 手动测试灯效：agent_status_light send --mode demo\n  2. 查看后台状态：agent_status_light status --verbose\n  3. 安装 Cursor Hook：agent_status_light install cursor\n  4. 查看排障日志：agent_status_light logs --limit 100\n  5. 关闭灯效：agent_status_light send --mode off\n\n更多普通用户说明见 docs/USER_MANUAL.md。"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 启动持有 BLE 连接的后台服务。
    #[command(
        long_about = "启动 AgentStatusLight 后台 daemon。\n\n通常不需要手动执行这个命令，因为 send 会在 daemon 不存在时自动尝试启动它。手动运行 daemon 主要用于调试蓝牙连接、查看实时日志，或者在当前终端里前台运行方便 Ctrl+C 停止。",
        after_help = "示例：\n  agent_status_light daemon\n  agent_status_light daemon --foreground\n\n说明：\n  不加 --foreground 时会尝试启动后台服务。\n  加 --foreground 时会在当前终端前台运行，适合调试。"
    )]
    Daemon {
        /// 在当前终端前台运行，方便调试和 Ctrl+C 中断。
        #[arg(
            long,
            help = "在当前终端前台运行 daemon",
            long_help = "在当前终端前台运行 daemon，方便查看实时日志和使用 Ctrl+C 中断。普通使用场景一般不需要这个参数。"
        )]
        foreground: bool,
    },

    /// 向 daemon 发送灯效模式。
    #[command(
        long_about = "向后台 daemon 发送一个灯效状态。\n\nsend 不直接操作 BLE，而是通过本地 IPC 把 mode 发送给 daemon。daemon 会根据 source/session 和优先级规则决定最终展示哪个灯效。\n\n手动执行 send --mode off 会清空所有 source/session，相当于全局关灯。Hook 中的 off 只清理对应 Agent 会话。",
        after_help = "示例：\n  agent_status_light send --mode demo\n  agent_status_light send --mode busy --source cursor --session my-session --ttl 1800\n  agent_status_light send --mode off\n\n常用 mode：\n  demo / thinking / ai / busy / success / error / alarm / traffic / off / red / yellow / green\n\n说明：\n  Hook 安装器会自动生成 --source、--session auto、--ttl、--quiet 和 --hook-id，一般用户不需要手写这些高级参数。"
    )]
    Send {
        /// 要发送的模式，例如 thinking、busy、success 或 off。
        #[arg(
            long,
            value_name = "MODE",
            help = "要发送的灯效模式",
            long_help = "要发送的灯效模式。支持：demo、thinking、ai、busy、success、error、alarm、traffic、off、red、yellow、green。"
        )]
        mode: String,

        /// 状态来源。Hook 会填写 codex/cursor/claude；手动命令默认 manual。
        #[arg(
            long,
            default_value = "manual",
            value_name = "SOURCE",
            help = "状态来源，例如 codex、cursor、claude",
            long_help = "状态来源，用于区分不同 Agent。Hook 会自动填写 codex、cursor 或 claude。手动命令默认 manual。daemon 会按 source + session 维护状态池。"
        )]
        source: String,

        /// 会话 ID。Hook 使用 auto 从 stdin JSON 提取；手动命令默认 manual。
        #[arg(
            long,
            default_value = "manual",
            value_name = "SESSION",
            help = "会话 ID，Hook 通常使用 auto",
            long_help = "会话 ID，用于区分同一个 Agent 的多个会话。Hook 通常使用 auto，工具会从 Hook stdin JSON 中提取 session_id、conversation_id、thread_id、tabId 等字段；没有时会用 cwd/workspace/transcript_path 等生成稳定哈希。手动命令默认 manual。"
        )]
        session: String,

        /// 状态在 daemon 内保留的秒数。不填时按 mode 使用默认 TTL。
        #[arg(
            long,
            value_name = "SECONDS",
            help = "状态保留秒数",
            long_help = "状态在 daemon 内保留的秒数。超过时间后会自动过期并回落到其它仍然活跃的状态。不填写时按 mode 使用默认 TTL。取值范围：1 到 86400。"
        )]
        ttl: Option<u64>,

        /// 静默模式。Hook 默认使用它，避免 stderr warning 污染 agent 输出。
        #[arg(
            long,
            help = "静默模式，不向 stderr 输出 warning",
            long_help = "静默模式。发送失败时默认不向 stderr 输出 warning，适合 Hook 调用，避免污染 Agent 输出。配合 --strict 使用时，失败仍会返回非 0。"
        )]
        quiet: bool,

        /// Hook 安装器写入的隐藏标记，用于卸载时更精准识别本工具条目。
        #[arg(long, hide = true)]
        hook_id: Option<String>,

        /// 发送失败时返回非 0 退出码。
        #[arg(
            long,
            help = "发送失败时返回非 0 退出码",
            long_help = "严格模式。默认情况下 send 失败只会 warning 并返回 0，避免 Hook 阻塞 Agent 主流程。加上 --strict 后，发送失败会返回非 0，适合手动排障或脚本中强校验。"
        )]
        strict: bool,
    },

    /// 查看 daemon 和 BLE 状态。
    #[command(
        long_about = "查看 AgentStatusLight 后台 daemon、BLE 连接和当前灯效状态。\n\n普通状态只展示 daemon、BLE、device、mode 等摘要。加 --verbose 后，会展示 source/session 状态池、优先级和剩余过期时间，适合排查多个 Agent 同时工作时为什么显示某个灯效。",
        after_help = "示例：\n  agent_status_light status\n  agent_status_light status --verbose\n\n字段说明：\n  daemon: 后台服务是否运行\n  ble: 蓝牙连接状态\n  mode: 最近写入设备的灯效\n  effective: 优先级路由当前计算出的灯效\n  sources: 当前仍然活跃的 source/session 状态池"
    )]
    Status {
        /// 展示 source/session 状态池和过期时间。
        #[arg(
            long,
            help = "展示 source/session 状态池",
            long_help = "展示详细状态池，包括每个 source/session 的 mode、priority 和 expires_in。用于排查多 Agent、多会话状态优先级。"
        )]
        verbose: bool,
    },

    /// 查看最近的 AgentStatusLight 事件日志。
    #[command(
        long_about = "查看 AgentStatusLight 最近事件日志。\n\n日志用于排查 daemon 启动、IPC 请求、BLE 扫描、连接、写入、重连、Hook 安装和卸载等关键流程。日志最多保留最近 1000 条，最新日志排在最上面。",
        after_help = "示例：\n  agent_status_light logs\n  agent_status_light logs --limit 200\n\n建议排障时同时提供：\n  agent_status_light status --verbose\n  agent_status_light logs --limit 100"
    )]
    Logs {
        /// 展示最近多少条日志，超过 1000 会自动截断。
        #[arg(
            long,
            default_value_t = 100,
            value_name = "N",
            help = "展示最近 N 条日志",
            long_help = "展示最近 N 条日志。超过 1000 会自动截断为 1000。默认展示 100 条。"
        )]
        limit: usize,
    },

    /// 停止后台 daemon。
    #[command(
        long_about = "停止 AgentStatusLight 后台 daemon。\n\n普通 stop 会通过 IPC 请求 daemon 优雅退出，并尽量发送 off 让灯关闭。只有在 daemon 无响应或 IPC 异常时，才需要使用 --force。",
        after_help = "示例：\n  agent_status_light stop\n  agent_status_light stop --force\n\n说明：\n  --force 是兜底手段，不保证 daemon 有机会发送 off 或清理 BLE 状态。"
    )]
    Stop {
        /// daemon 无法通过 IPC 停止时，根据 pid 文件强制停止。
        #[arg(
            long,
            help = "强制停止 daemon",
            long_help = "根据 pid 文件强制停止 daemon。仅在普通 stop 无法停止时使用。"
        )]
        force: bool,
    },

    /// 为指定 agent 安装可直接使用的 Hook 配置。
    #[command(
        long_about = "为 Codex、Cursor 或 Claude 安装可直接使用的 AgentStatusLight Hook 配置。\n\n不填写 --dir 时安装到用户全局配置：\n  Codex:  ~/.codex/hooks.json\n  Cursor: ~/.cursor/hooks.json\n  Claude: ~/.claude/settings.json\n\n填写 --dir 时安装到项目级配置：\n  Codex:  <dir>/.codex/hooks.json\n  Cursor: <dir>/.cursor/hooks.json\n  Claude: <dir>/.claude/settings.json\n\n安装时会把当前 AgentStatusLight 可执行文件复制到固定目录的 bin 下，Hook 会引用这份稳定副本，避免用户删除解压包后失效。固定目录：macOS/Linux 为 ~/.agent-status-light，Windows 为 C:\\.agent-status-light。",
        after_help = "示例：\n  agent_status_light install cursor\n  agent_status_light install codex\n  agent_status_light install claude\n  agent_status_light install cursor --dir .\n\n说明：\n  重复执行 install 是安全的，会先清理旧的 AgentStatusLight Hook 条目，再写入新的配置。"
    )]
    Install {
        #[arg(
            value_enum,
            value_name = "TARGET",
            help = "Hook 目标：codex、cursor 或 claude"
        )]
        target: HookTarget,

        /// 项目目录。不填写时安装到用户全局配置目录。
        #[arg(
            long,
            value_name = "DIR",
            help = "项目目录；不填则安装到用户全局配置",
            long_help = "项目目录。填写后会安装到该项目的 .codex/.cursor/.claude 配置目录；不填写则安装到用户全局配置目录。"
        )]
        dir: Option<PathBuf>,
    },

    /// 卸载指定 agent 的 AgentStatusLight Hook 配置。
    #[command(
        long_about = "卸载 Codex、Cursor 或 Claude 中的 AgentStatusLight Hook 配置。\n\n卸载只会移除 AgentStatusLight 自己安装的 Hook 条目，不会删除用户手写的其它 Hook。新版本 Hook 会带 --hook-id agent-status-light 标记，卸载时会优先按这个标记识别。",
        after_help = "示例：\n  agent_status_light uninstall cursor\n  agent_status_light uninstall codex\n  agent_status_light uninstall claude\n  agent_status_light uninstall cursor --dir .\n\n说明：\n  install 时使用了 --dir，uninstall 时也需要使用同一个 --dir。"
    )]
    Uninstall {
        #[arg(
            value_enum,
            value_name = "TARGET",
            help = "Hook 目标：codex、cursor 或 claude"
        )]
        target: HookTarget,

        /// 项目目录。不填写时从用户全局配置目录卸载。
        #[arg(
            long,
            value_name = "DIR",
            help = "项目目录；不填则从用户全局配置卸载",
            long_help = "项目目录。填写后会从该项目的 .codex/.cursor/.claude 配置中卸载；不填写则从用户全局配置中卸载。"
        )]
        dir: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum HookTarget {
    Codex,
    Cursor,
    Claude,
}

impl HookTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Claude => "claude",
        }
    }
}
