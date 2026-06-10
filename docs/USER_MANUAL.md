# AgentStatusLight 普通用户使用手册

这份手册写给“不懂代码”的用户。你拿到的是一个已经烧录好固件的 AgentStatusLight 设备，只需要把它插到电脑旁边，再安装电脑端小工具，就可以让 Codex、Cursor、Claude 的工作状态自动显示到状态灯上。

## 1. 你会拿到什么

你应该会收到两样东西：

1. 一个已经烧录好的 AgentStatusLight 设备。
2. 一个电脑端工具压缩包。

电脑端工具压缩包里通常会有：

```text
agent_status_light        # macOS / Linux 使用
agent_status_light.exe    # Windows 使用
README 或使用说明
```

如果你只收到了设备，没有收到电脑端工具，请联系提供设备的人索要。

## 2. 先把设备插上电脑

1. 用 USB 数据线把 AgentStatusLight 插到电脑上。
2. 等待几秒钟。
3. 设备会自动通过蓝牙广播，名称是：

```text
AgentStatusLight
```

电脑端工具会自动寻找这个名字的蓝牙设备。

> 注意：USB 主要负责供电；电脑端工具通过 BLE 蓝牙控制设备。

## 3. 安装电脑端工具

### macOS

1. 解压你收到的工具包。
2. 找到文件：

```text
agent_status_light
```

3. 打开“终端”。
4. 把 `agent_status_light` 拖进终端窗口，后面输入：

```bash
 status
```

最终看起来类似：

```bash
/你的路径/agent_status_light status
```

5. 回车执行。

如果 macOS 提示“无法打开，因为无法验证开发者”，可以：

1. 打开“系统设置”。
2. 进入“隐私与安全性”。
3. 找到刚才被拦截的 `agent_status_light`。
4. 点击“仍要打开”。

如果终端提示没有执行权限，可以执行：

```bash
chmod +x /你的路径/agent_status_light
```

不会操作的话，把这一步截图发给提供设备的人处理即可。

### Windows

1. 解压你收到的工具包。
2. 找到文件：

```text
agent_status_light.exe
```

3. 在文件夹空白处按住 Shift，点击鼠标右键。
4. 选择“在终端中打开”或“在 PowerShell 中打开”。
5. 执行：

```powershell
.\agent_status_light.exe status
```

如果 Windows 安全提示拦截，请选择“仍要运行”。

## 4. 第一次测试设备

### macOS / Linux

在终端执行：

```bash
/你的路径/agent_status_light send --mode demo
```

关闭灯效：

```bash
/你的路径/agent_status_light send --mode off
```

### Windows

在 PowerShell 执行：

```powershell
.\agent_status_light.exe send --mode demo
```

关闭灯效：

```powershell
.\agent_status_light.exe send --mode off
```

如果灯亮了，说明设备和电脑端工具已经可以通信。

第一次执行时，工具会自动启动一个后台服务。这个后台服务负责一直连接蓝牙设备，后续 Agent Hook 触发时会很快响应。

## 5. 安装到 Codex / Cursor / Claude

电脑端工具支持三个目标：

```text
codex
cursor
claude
```

你用哪个工具，就安装哪个。

### 5.1 安装到 Codex

macOS / Linux：

```bash
/你的路径/agent_status_light install codex
```

Windows：

```powershell
.\agent_status_light.exe install codex
```

### 5.2 安装到 Cursor

macOS / Linux：

```bash
/你的路径/agent_status_light install cursor
```

Windows：

```powershell
.\agent_status_light.exe install cursor
```

### 5.3 安装到 Claude

macOS / Linux：

```bash
/你的路径/agent_status_light install claude
```

Windows：

```powershell
.\agent_status_light.exe install claude
```

安装完成后，重新打开对应的 Agent 工具，或者重新开始一个会话。

## 6. 安装后会发生什么

当 Agent 工作时，状态灯会自动变化。

常见状态：

| Agent 状态       | 灯效       |
| ---------------- | ---------- |
| 开始分析 / 思考  | `thinking` |
| 正在生成或改代码 | `ai`       |
| 正在执行命令     | `busy`     |
| 成功             | `success`  |
| 失败             | `error`    |
| 等待你审批或操作 | `alarm`    |
| 严重异常 / 阻塞  | `alarm`    |
| 结束或关闭       | `off`      |

如果你同时使用多个 Agent，或者同时打开多个会话，工具会自动判断优先级。比如一个会话正在 `busy`，另一个会话刚刚 `success`，状态灯会优先显示 `busy`。

## 7. 查看当前状态

### macOS / Linux

```bash
/你的路径/agent_status_light status
```

查看更详细的信息：

```bash
/你的路径/agent_status_light status --verbose
```

### Windows

```powershell
.\agent_status_light.exe status
```

详细信息：

```powershell
.\agent_status_light.exe status --verbose
```

你可能会看到：

```text
daemon: running
ble: connected
mode: busy
effective: busy
```

含义：

- `daemon: running`：后台服务正在运行。
- `ble: connected`：已经连接到设备。
- `mode`：最近发送给设备的灯效。
- `effective`：当前优先级计算后应该展示的灯效。

## 8. 查看日志

如果灯没有按预期变化，可以查看日志。

macOS / Linux：

```bash
/你的路径/agent_status_light logs --limit 100
```

Windows：

```powershell
.\agent_status_light.exe logs --limit 100
```

把日志截图发给提供设备的人，通常就能快速定位问题。

## 9. 手动控制灯效

你也可以手动设置灯效。

macOS / Linux：

```bash
/你的路径/agent_status_light send --mode thinking
/你的路径/agent_status_light send --mode busy
/你的路径/agent_status_light send --mode success
/你的路径/agent_status_light send --mode error
/你的路径/agent_status_light send --mode off
```

Windows：

```powershell
.\agent_status_light.exe send --mode thinking
.\agent_status_light.exe send --mode busy
.\agent_status_light.exe send --mode success
.\agent_status_light.exe send --mode error
.\agent_status_light.exe send --mode off
```

支持的 mode：

```text
demo / thinking / ai / busy / success / error / alarm / traffic / off / red / yellow / green
```

## 10. 停止后台服务

通常不需要手动停止后台服务。

如果你想停止：

macOS / Linux：

```bash
/你的路径/agent_status_light stop
```

Windows：

```powershell
.\agent_status_light.exe stop
```

如果提示服务无法响应，可以强制停止：

macOS / Linux：

```bash
/你的路径/agent_status_light stop --force
```

Windows：

```powershell
.\agent_status_light.exe stop --force
```

## 11. 卸载 Hook

如果你不想让某个 Agent 继续控制状态灯，可以卸载对应 Hook。

### Codex

macOS / Linux：

```bash
/你的路径/agent_status_light uninstall codex
```

Windows：

```powershell
.\agent_status_light.exe uninstall codex
```

### Cursor

macOS / Linux：

```bash
/你的路径/agent_status_light uninstall cursor
```

Windows：

```powershell
.\agent_status_light.exe uninstall cursor
```

### Claude

macOS / Linux：

```bash
/你的路径/agent_status_light uninstall claude
```

Windows：

```powershell
.\agent_status_light.exe uninstall claude
```

卸载只会移除 AgentStatusLight 自己安装的配置，不会删除你原本已有的其它配置。

## 12. 常见问题

### 12.1 设备没有反应

按顺序检查：

1. 设备是否插电。
2. 电脑蓝牙是否打开。
3. 设备是否离电脑太远。
4. 是否执行过测试命令：

macOS / Linux：

```bash
/你的路径/agent_status_light send --mode demo
```

Windows：

```powershell
.\agent_status_light.exe send --mode demo
```

5. 查看状态：

```bash
agent_status_light status --verbose
```

如果 `ble` 不是 `connected`，说明电脑还没有连上设备。

### 12.2 macOS 提示没有权限

如果 macOS 拦截工具：

1. 打开“系统设置”。
2. 进入“隐私与安全性”。
3. 找到 `agent_status_light`。
4. 点击“仍要打开”。

如果是蓝牙权限问题，请允许终端或当前应用访问蓝牙。

### 12.3 Cursor / Codex / Claude 没有触发灯效

尝试：

1. 重新执行对应 install 命令。
2. 重启对应 Agent 工具。
3. 开启一个新的会话。
4. 查看状态：

```bash
agent_status_light status --verbose
```

5. 查看日志：

```bash
agent_status_light logs --limit 100
```

### 12.4 灯一直停在某个状态

可以手动关闭：

```bash
agent_status_light send --mode off
```

如果后台服务异常，可以停止它：

```bash
agent_status_light stop
```

## 13. 最简单的使用流程

普通用户只需要记住这几步：

1. 插上 AgentStatusLight 设备。
2. 解压电脑端工具。
3. 测试灯效：

```bash
agent_status_light send --mode demo
agent_status_light send --mode off
```

4. 安装到自己使用的 Agent：

```bash
agent_status_light install cursor
```

或者：

```bash
agent_status_light install codex
agent_status_light install claude
```

5. 开始使用 Agent。

遇到问题时执行：

```bash
agent_status_light status --verbose
agent_status_light logs --limit 100
```
