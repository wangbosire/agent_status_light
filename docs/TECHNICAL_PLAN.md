# AgentStatusLight 技术方案

本文档用于描述电脑端 Rust 脚本的可执行实现方案。当前 ESP32-C3 Arduino 固件已经提供 BLE GATT 写入能力，电脑端需要补齐 CLI、后台服务、Hook 安装和卸载能力。

## 1. 目标

AgentStatusLight 的电脑端程序需要满足以下目标：

- Hook 触发时快速返回，不在 Hook 进程中执行 BLE 扫描和连接。
- 通过后台 daemon 维护 BLE 连接，并负责自动重连。
- `send` 命令只向 daemon 发送本地指令。
- 提供 `stop` 命令，允许用户优雅停止后台服务。
- `install` / `uninstall` 支持不传 `--dir`，默认使用用户全局配置目录。
- 支持项目级安装，传入 `--dir` 时写入指定项目目录。

## 2. 总体架构

```text
Codex / Cursor / Claude Hook
  -> agent_status_light send --mode busy
      -> 本地 IPC
          -> agent_status_light daemon
              -> 常驻扫描 / 连接 ESP32-C3
              -> 写入 BLE characteristic
              -> 断线自动重连
```

Hook 只调用 `send`。`send` 不直接操作 BLE，而是把模式指令发送给本机 daemon。daemon 是唯一直接连接 ESP32-C3 的进程。

## 3. 固件协议

电脑端需要和固件保持一致的 BLE 参数：

```text
Device Name: AgentStatusLight
Service UUID: b8b7e001-7a6b-4f4f-9a8b-11c0ffee0001
Mode Characteristic UUID: b8b7e002-7a6b-4f4f-9a8b-11c0ffee0001
```

注意：当前固件中的 BLE 名称如果仍是 `CursorLight`，需要改成 `AgentStatusLight`，同时同步串口日志，避免脚本按 README 扫描时找不到设备。

支持的 mode：

```text
demo
thinking
ai
busy
success
error
alarm
traffic
off
red
yellow
green
```

写入 characteristic 的内容就是 mode 字符串，例如：

```text
busy
success
off
```

## 4. CLI 设计

建议支持以下命令：

```bash
agent_status_light daemon
agent_status_light daemon --foreground
agent_status_light send --mode thinking
agent_status_light send --mode busy --strict
agent_status_light status
agent_status_light logs --limit 100
agent_status_light stop
agent_status_light stop --force
agent_status_light install codex
agent_status_light install cursor
agent_status_light install claude
agent_status_light install codex --dir .
agent_status_light uninstall codex
agent_status_light uninstall codex --dir .
```

### 4.1 `daemon`

启动后台服务。

默认行为：

- 检查是否已有 daemon 运行。
- 创建运行时目录。
- 写入 pid 文件。
- 启动本地 IPC server。
- 启动 BLE manager。
- 后台运行。

`--foreground` 用于调试：

- 不脱离当前终端。
- 日志直接输出到终端。
- 支持 `Ctrl+C` 中断。

### 4.2 `send`

发送灯效模式。

示例：

```bash
agent_status_light send --mode busy
```

行为：

- 校验 mode 是否合法。
- 尝试连接本地 daemon。
- 如果 daemon 已运行，发送 IPC 指令。
- 如果 daemon 未运行，尝试自动启动 daemon，然后重试一次。
- 默认失败时打印 warning，但退出码为 0，避免 Hook 阻塞主流程。
- 如果带 `--strict`，失败时返回非 0。

### 4.3 `status`

查看状态。

建议输出：

```text
daemon: running
pid: 12345
ipc: 127.0.0.1:47631
ble: connected
device: AgentStatusLight
mode: busy
```

daemon 未运行时：

```text
daemon: stopped
```

### 4.4 `logs`

查看最近的关键流程日志。

示例：

```bash
agent_status_light logs --limit 100
```

行为：

- 从 runtime 目录读取结构化事件日志。
- 默认展示最近 100 条。
- 最大支持展示 1000 条。
- 日志文件本身最多保留最近 1000 条，避免无限增长。
- 日志按倒序写入，最新日志在文件最上面。
- 写入由 daemon 内部日志 worker 串行执行，不创建额外的 `events.lock` 文件。
- 日志 worker 启动时加载最近日志到内存，后续维护内存缓存并刷新 `events.jsonl`。
- daemon 退出时会关闭日志队列并等待 worker flush，避免最后几条日志丢失。
- 读取时会跳过损坏的日志行，避免单行问题导致整个日志不可读。

日志文件：

```text
~/.agent-status-light/runtime/events.jsonl
```

### 4.5 `stop`

优雅停止 daemon。

行为：

- 连接本地 IPC。
- 发送 shutdown 指令。
- daemon 收到后尝试发送 `off`。
- daemon 断开 BLE。
- daemon 关闭 IPC server。
- daemon 清理 pid / lock 文件。
- daemon 退出。

### 4.6 `stop --force`

兜底停止命令。

行为：

- 读取 pid 文件。
- 检查进程是否存在。
- 发送系统终止信号。
- 清理 stale pid / lock 文件。

`--force` 只用于 daemon 无响应、IPC 无法连接等异常情况。

### 4.7 `install`

安装真实可用的 Codex / Cursor / Claude Hook 配置和辅助文件。

不传 `--dir`：

```bash
agent_status_light install codex
```

写入用户全局 agent 配置：

```text
Codex:  ~/.codex/hooks.json
Cursor: ~/.cursor/hooks.json
Claude: ~/.claude/settings.json
```

传 `--dir`：

```bash
agent_status_light install codex --dir .
```

写入指定项目目录下的 agent 配置：

```text
Codex:  <dir>/.codex/hooks.json
Cursor: <dir>/.cursor/hooks.json
Claude: <dir>/.claude/settings.json
```

`.agent-status-light/` 保存 AgentStatusLight 的稳定二进制副本、runtime 和安装清单。

### 4.8 `uninstall`

卸载 AgentStatusLight 安装到 Codex / Cursor / Claude 配置中的 Hook 条目。

不传 `--dir`：

```bash
agent_status_light uninstall codex
```

从用户全局 agent 配置中移除命令包含 `agent_status_light send --mode` 的 Hook 条目。

传 `--dir`：

```bash
agent_status_light uninstall codex --dir .
```

从指定项目目录的 agent 配置中移除命令包含 `agent_status_light send --mode` 的 Hook 条目。

## 5. 目录设计

### 5.1 全局安装目录

AgentStatusLight 自己的文件统一放在固定目录，和目标 Agent 的 Hook 配置文件分开。

参考路径：

```text
macOS:   ~/.agent-status-light/
Linux:   ~/.agent-status-light/
Windows: C:\.agent-status-light\
```

目录结构：

```text
agent-status-light/
├─ bin/
├─ runtime/
└─ config.[codex/cursor/claude].json
```

全局 Hook 配置写入目标工具自己的目录：

```text
~/.codex/hooks.json
~/.cursor/hooks.json
~/.claude/settings.json
```

### 5.2 项目安装目录

传入 `--dir` 时，只改变目标 Agent 的 Hook 配置写入位置；AgentStatusLight 自己的 runtime 和安装清单仍然放在固定目录。

项目级 Hook 配置写入：

```text
<dir>/.codex/hooks.json
<dir>/.cursor/hooks.json
<dir>/.claude/settings.json
```

项目级安装不创建项目内 runtime。

### 5.3 运行时目录

daemon 是用户级服务，不属于单个项目。因此运行时目录始终位于固定目录：

```text
~/.agent-status-light/runtime/
├─ daemon.pid
├─ daemon.log
├─ events.jsonl
├─ ipc.json
└─ token
```

多个项目的 Hook 都会发送到同一个 daemon。

## 6. IPC 方案

第一版使用本地 TCP，跨平台简单稳定：

```text
127.0.0.1:47631
```

消息格式使用 JSON Lines，一行一个请求。

请求示例：

```json
{"token":"...","cmd":"send","mode":"busy"}
{"token":"...","cmd":"status"}
{"token":"...","cmd":"shutdown"}
```

响应示例：

```json
{"ok":true}
{"ok":true,"daemon":"running","ble":"connected","mode":"busy"}
{"ok":false,"error":"device_not_found"}
```

token 存储在：

```text
~/.agent-status-light/runtime/token
```

token 用于避免本机其它进程误发指令。第一版可以实现简单随机 token，daemon 和 client 读取同一份文件。

## 7. BLE manager 设计

daemon 内部维护一个 BLE manager。

建议状态：

```rust
enum BleState {
    Idle,
    Scanning,
    Connecting,
    Connected,
    Disconnected,
    DeviceNotFound,
    Error(String),
}
```

职责：

- 扫描 BLE 设备。
- 匹配设备名 `AgentStatusLight`，或匹配 service UUID。
- 连接设备。
- discover service。
- discover mode characteristic。
- 接收 mode 指令。
- 写入 characteristic。
- 断线后自动重连。

写入策略：

- daemon 收到新 mode 后，如果已连接，立即写入。
- 如果正在重连，保存最近一次 mode，连接成功后写入最新 mode。
- 多个连续 mode 到来时，以最后一次为准。

重连策略：

- 初始扫描超时建议 5 秒。
- 连接失败后退避重试。
- 前几次可以快速重试，例如 1 秒、2 秒、3 秒。
- 后续维持 5 秒间隔，避免持续占用资源。

## 8. Hook 设计

Hook 配置里只调用 `send`：

```bash
/path/to/agent_status_light send --mode busy --source codex --session auto --ttl 1800 --quiet --hook-id agent-status-light
```

或全局安装时使用全局 bin 路径。

推荐映射：

```text
Agent 开始分析        -> thinking
Agent 正在生成/修改   -> ai
执行命令/测试/构建    -> busy
成功                 -> success
普通失败             -> error
等待用户审批或操作   -> alarm
严重阻塞             -> alarm
结束/收尾            -> off
```

多 Agent 优先级：

```text
alarm > error > yellow > busy > ai > thinking > success > red > green > demo > traffic > off
```

daemon 按 `source + session` 维护状态池：

- Codex Hook 使用 `--source codex --session auto`。
- Cursor Hook 使用 `--source cursor --session auto`。
- Claude Hook 使用 `--source claude --session auto`。
- 手动命令默认 `--source manual`。

`--session auto` 会读取 Hook stdin JSON，优先提取 `session_id` / `sessionId` / `conversation_id` / `thread_id` / `tabId` 等字段；如果目标工具没有提供会话 ID，则使用 `cwd` / `workspace_path` / `transcript_path` 等稳定上下文字段生成哈希作为兜底 session。

普通 Hook 的 `off` 只清除对应 source/session；手动执行 `agent_status_light send --mode off` 会清空所有 source/session。每个 mode 在 daemon 内都有 TTL，高优先级状态过期后会自动回落到仍然活跃的低优先级状态。

Hook 默认使用 `--quiet` 避免 warning 污染 agent 输出，使用 `--hook-id agent-status-light` 作为卸载标记。排障时使用 `agent_status_light status --verbose` 查看当前所有活跃 source/session、优先级和剩余过期时间。

已对齐的 Hook schema：

- Codex：`hooks.json` 顶层为 `hooks`，事件名使用 `SessionStart` / `PreToolUse` / `PostToolUse` / `Stop`，事件项内使用 `matcher` 与 `hooks: [{ type: "command", command, timeout }]`。
- Cursor：`hooks.json` 顶层包含 `version: 1` 与 `hooks`，事件名使用 `sessionStart` / `beforeSubmitPrompt` / `preToolUse` / `beforeShellExecution` / `afterShellExecution` / `stop` / `sessionEnd`，事件项直接包含 `command`、`timeout`、`matcher`、`failClosed`。
- Claude：`settings.json` 顶层为 `hooks`，事件结构与 Codex 类似，使用 `SessionStart` / `UserPromptSubmit` / `PreToolUse` / `PostToolUse` / `Notification` / `Stop` / `SubagentStop`。

安装路径：

```text
Codex 全局：  ~/.codex/hooks.json
Codex 项目：  <dir>/.codex/hooks.json
Cursor 全局： ~/.cursor/hooks.json
Cursor 项目： <dir>/.cursor/hooks.json
Claude 全局： ~/.claude/settings.json
Claude 项目： <dir>/.claude/settings.json
```

安装时会读取已有 JSON，先清理旧的 AgentStatusLight 条目，再追加新的条目，因此重复执行 `install` 不会产生重复 Hook。卸载时只删除命令中包含 `agent_status_light send --mode` 的条目。

## 9. Rust 模块划分

建议结构：

```text
src/
├─ main.rs
├─ cli.rs
├─ config.rs
├─ modes.rs
├─ ipc.rs
├─ daemon.rs
├─ ble.rs
├─ install.rs
└─ hooks.rs
```

职责：

- `main.rs`：入口，初始化日志，分发 CLI。
- `cli.rs`：`clap` 参数定义。
- `config.rs`：路径、配置、token、pid 文件。
- `modes.rs`：合法 mode 定义和校验。
- `ipc.rs`：TCP JSON Lines client/server。
- `daemon.rs`：daemon 生命周期、信号处理、shutdown。
- `ble.rs`：BLE 扫描、连接、写入、重连。
- `install.rs`：安装和卸载 Hook 配置，维护安装清单。
- `hooks.rs`：生成、合并和清理 Codex / Cursor / Claude Hook 配置。

## 10. 依赖建议

```toml
[dependencies]
anyhow = "1"
btleplug = "0.11"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = "1"
```

版本可以在实际实现时根据当前可用 crate 调整。

## 11. 实现阶段

### 阶段一：协议和 CLI 骨架

- 修正固件 BLE 名称。
- 引入 CLI 参数解析。
- 定义 mode 校验。
- 实现 `send/status/stop/daemon/install/uninstall` 命令骨架。

验收：

```bash
agent_status_light send --mode busy
agent_status_light status
agent_status_light stop
```

命令能解析，错误参数有清晰提示。

### 阶段二：IPC 和 daemon 生命周期

- 实现本地 TCP server。
- 实现 JSON Lines client。
- 实现 pid 文件。
- 实现 token。
- 实现 `status`。
- 实现 `stop`。
- 实现 `stop --force`。

验收：

```bash
agent_status_light daemon --foreground
agent_status_light status
agent_status_light stop
```

daemon 可以启动、查询、停止。

### 阶段三：BLE manager

- 使用 `btleplug` 扫描设备。
- 连接 `AgentStatusLight`。
- discover service 和 characteristic。
- 写入 mode。
- 实现断线重连。

验收：

```bash
agent_status_light daemon --foreground
agent_status_light send --mode success
agent_status_light send --mode error
agent_status_light send --mode off
```

ESP32 灯效能正确变化。

### 阶段四：send 自动拉起 daemon

- `send` 连接 IPC 失败时自动启动 daemon。
- 等待 daemon ready。
- 重试发送。
- 默认失败不阻塞 Hook。
- `--strict` 失败返回非 0。

验收：

```bash
agent_status_light stop
agent_status_light send --mode busy
agent_status_light status
```

daemon 被自动拉起，mode 能发送。

### 阶段五：install / uninstall

- 实现全局安装目录。
- 实现项目级安装目录。
- 复制当前可执行文件到固定目录的 `bin/`。
- 生成并合并真实 Hook 配置。
- 实现 uninstall。

验收：

```bash
agent_status_light install codex
agent_status_light uninstall codex
agent_status_light install codex --dir .
agent_status_light uninstall codex --dir .
```

全局和项目级目录行为符合预期。

### 阶段六：文档和排障

- 更新 README。
- 补充 macOS / Windows / Linux 使用说明。
- 补充 BLE 找不到设备的排障。
- 补充 daemon 无法停止的排障。
- 补充 Hook 配置安装和卸载说明。

## 12. 关键验收场景

### 手动控制

```bash
agent_status_light daemon --foreground
agent_status_light send --mode demo
agent_status_light send --mode thinking
agent_status_light send --mode busy
agent_status_light send --mode success
agent_status_light send --mode error
agent_status_light send --mode off
```

### 后台服务停止

```bash
agent_status_light daemon
agent_status_light status
agent_status_light stop
agent_status_light status
```

预期：daemon 从 running 变为 stopped。

### 自动拉起

```bash
agent_status_light stop
agent_status_light send --mode busy
agent_status_light status
```

预期：`send` 自动启动 daemon。

### 项目级安装

```bash
agent_status_light install codex --dir .
```

预期：

```text
./.codex/hooks.json
~/.agent-status-light/config.codex.json
```

### 全局安装

```bash
agent_status_light install codex
```

预期：文件出现在用户全局配置目录。

## 13. 风险和注意事项

- BLE 设备名必须和固件一致。
- macOS 首次使用 BLE 可能需要蓝牙权限。
- Windows BLE 行为可能依赖系统蓝牙栈和适配器能力。
- Hook 中不应使用会长时间阻塞的命令。
- daemon 必须提供 `stop`，否则用户难以中断后台连接。
- `send` 默认不要让 Hook 失败，除非用户显式使用 `--strict`。
- 多个项目可能同时触发 Hook，daemon 需要能接收并处理连续指令。
