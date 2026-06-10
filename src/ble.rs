//! BLE 连接管理。
//!
//! 这个模块只在 daemon 内运行，负责扫描、连接 ESP32-C3，并把 mode 写入 GATT characteristic。
//! 它通过 channel 接收 mode，而不是暴露同步写入函数；
//! 这样 IPC 请求可以快速返回，BLE 扫描、连接和重连都留在后台任务里完成。

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType},
    platform::{Manager, Peripheral},
};
use tokio::{
    sync::{RwLock, mpsc},
    time,
};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::log_store::{self, LogSender};

pub const DEVICE_NAME: &str = "AgentStatusLight";
pub const SERVICE_UUID: &str = "b8b7e001-7a6b-4f4f-9a8b-11c0ffee0001";
pub const MODE_CHAR_UUID: &str = "b8b7e002-7a6b-4f4f-9a8b-11c0ffee0001";

#[derive(Debug, Clone)]
pub struct BleSnapshot {
    pub state: String,
    pub device: Option<String>,
    pub mode: Option<String>,
    pub last_error: Option<String>,
}

impl BleSnapshot {
    pub fn new() -> Self {
        Self {
            state: "idle".to_owned(),
            device: None,
            mode: None,
            last_error: None,
        }
    }
}

pub type SharedBleStatus = Arc<RwLock<BleSnapshot>>;

/// 已连接设备和目标 characteristic 的组合，写入 mode 时只需要这个结构。
struct ConnectedDevice {
    peripheral: Peripheral,
    characteristic: btleplug::api::Characteristic,
}

pub fn spawn_manager(
    mut rx: mpsc::Receiver<String>,
    status: SharedBleStatus,
    log_tx: LogSender,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut connected: Option<ConnectedDevice> = None;
        // 保存最近一次 mode。重连成功后会重放最新 mode，避免短暂断线导致状态丢失。
        let mut latest_mode: Option<String> = None;
        // 周期性重连。tokio interval 的首次 tick 会很快触发，因此 daemon 启动后会主动连接。
        let mut reconnect_interval = time::interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                Some(mode) = rx.recv() => {
                    // 收到 mode 后先更新快照。即使蓝牙暂时不可用，status 也能看到最近请求。
                    latest_mode = Some(mode.clone());
                    set_mode_status(&status, Some(mode.clone())).await;
                    log_store::emit(&log_tx, "info", "ble.mode_queued", format!("mode={mode}"));

                    if connected.is_none() {
                        // 当前没有连接时，按需扫描并连接设备。
                        connected = connect(&status, &log_tx).await.ok();
                    }

                    if let Some(device) = connected.as_ref() {
                        // 写入失败通常表示设备断开或系统蓝牙栈出错。
                        // 丢弃旧连接，等待下一次重连重新 discover characteristic。
                        if let Err(err) = write_mode(device, &mode).await {
                            warn!("failed to write BLE mode: {err:#}");
                                log_store::emit(&log_tx, "warn", "ble.write_failed", err.to_string());
                            set_error(&status, "disconnected", Some(err.to_string())).await;
                            connected = None;
                        } else {
                            log_store::emit(&log_tx, "info", "ble.write_ok", format!("mode={mode}"));
                        }
                    }
                }

                _ = reconnect_interval.tick() => {
                    if let Some(device) = connected.as_ref() {
                        match device.peripheral.is_connected().await {
                            Ok(true) => {}
                            Ok(false) => {
                                log_store::emit(&log_tx, "warn", "ble.disconnected", "device disconnected");
                                set_error(&status, "disconnected", Some("device disconnected".to_owned())).await;
                                connected = None;
                            }
                            Err(err) => {
                                log_store::emit(&log_tx, "warn", "ble.connection_check_failed", err.to_string());
                                set_error(&status, "disconnected", Some(err.to_string())).await;
                                connected = None;
                            }
                        }
                    }

                    if connected.is_none() {
                        // 后台保持低频重连，避免设备重启或蓝牙短暂不可用后需要手动恢复。
                        connected = connect(&status, &log_tx).await.ok();

                        if let (Some(device), Some(mode)) = (connected.as_ref(), latest_mode.as_ref()) {
                            // 重连后写入最新 mode，让灯恢复到最近一次用户请求的状态。
                            if let Err(err) = write_mode(device, mode).await {
                                warn!("failed to replay BLE mode after reconnect: {err:#}");
                                log_store::emit(&log_tx, "warn", "ble.replay_failed", err.to_string());
                                set_error(&status, "disconnected", Some(err.to_string())).await;
                                connected = None;
                            } else {
                                log_store::emit(&log_tx, "info", "ble.replay_ok", format!("mode={mode}"));
                            }
                        }
                    }
                }
            }
        }
    })
}

/// 扫描并连接 ESP32-C3，成功后返回可直接写入的 BLE 句柄。
async fn connect(status: &SharedBleStatus, log_tx: &LogSender) -> Result<ConnectedDevice> {
    set_state(status, "scanning", None).await;
    log_store::emit(
        log_tx,
        "info",
        "ble.scanning",
        format!("device={DEVICE_NAME}"),
    );

    let service_uuid = Uuid::parse_str(SERVICE_UUID)?;
    let mode_uuid = Uuid::parse_str(MODE_CHAR_UUID)?;

    // btleplug 会根据平台选择系统蓝牙后端，例如 macOS CoreBluetooth。
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no BLE adapter found"))?;

    // 先做通用扫描，再按设备名或 service UUID 匹配。
    // 有些平台/设备不会稳定广播完整 service，所以两种方式任一命中即可。
    central.start_scan(ScanFilter::default()).await?;
    time::sleep(Duration::from_secs(5)).await;

    let peripherals = central.peripherals().await?;
    let _ = central.stop_scan().await;
    let mut matched: Option<Peripheral> = None;

    for peripheral in peripherals {
        let properties = peripheral.properties().await?;
        let Some(properties) = properties else {
            continue;
        };

        // 优先匹配 README 和固件约定的 BLE 名称。
        let name_matches = properties.local_name.as_deref() == Some(DEVICE_NAME);
        let service_matches = properties.services.contains(&service_uuid);

        if name_matches || service_matches {
            matched = Some(peripheral);
            break;
        }
    }

    let peripheral = match matched {
        Some(peripheral) => peripheral,
        None => {
            let message = format!("BLE device {DEVICE_NAME} not found");
            log_store::emit(log_tx, "warn", "ble.device_not_found", &message);
            return Err(anyhow!(message));
        }
    };

    set_state(status, "connecting", Some(DEVICE_NAME.to_owned())).await;
    log_store::emit(
        log_tx,
        "info",
        "ble.connecting",
        format!("device={DEVICE_NAME}"),
    );
    if !peripheral.is_connected().await? {
        // 如果系统已经保持连接，btleplug 会直接复用；否则主动 connect。
        peripheral.connect().await?;
    }
    peripheral.discover_services().await?;

    // discover 后从 characteristic 列表中找到 mode 写入点。
    let characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == mode_uuid)
        .ok_or_else(|| anyhow!("mode characteristic {MODE_CHAR_UUID} not found"))?;

    set_state(status, "connected", Some(DEVICE_NAME.to_owned())).await;
    log_store::emit(
        log_tx,
        "info",
        "ble.connected",
        format!("device={DEVICE_NAME}"),
    );
    debug!("connected to BLE device {DEVICE_NAME}");

    Ok(ConnectedDevice {
        peripheral,
        characteristic,
    })
}

/// 向 mode characteristic 写入纯文本 mode。
async fn write_mode(device: &ConnectedDevice, mode: &str) -> Result<()> {
    device
        .peripheral
        .write(
            &device.characteristic,
            mode.as_bytes(),
            WriteType::WithResponse,
        )
        .await
        .with_context(|| format!("failed to write mode {mode}"))?;
    Ok(())
}

async fn set_state(status: &SharedBleStatus, state: &str, device: Option<String>) {
    // 状态快照供 `agent_status_light status` 读取，因此只放轻量信息。
    let mut status = status.write().await;
    status.state = state.to_owned();
    status.device = device;
    status.last_error = None;
}

async fn set_error(status: &SharedBleStatus, state: &str, error: Option<String>) {
    // 错误不会让 daemon 退出，只会反映到 status 和日志里，等待后续重连恢复。
    let mut status = status.write().await;
    status.state = state.to_owned();
    status.last_error = error;
}

async fn set_mode_status(status: &SharedBleStatus, mode: Option<String>) {
    // 记录最近请求的 mode，便于用户排查当前灯效来源。
    let mut status = status.write().await;
    status.mode = mode;
}
