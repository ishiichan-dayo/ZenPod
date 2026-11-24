//! iPod を探す・開く・取り出す

use std::path::PathBuf;

use ipod_core::library::Mode;
use ipod_core::t;
use tauri::{AppHandle, State, Window};
use zenpod_app::application::devices::DeviceEntry;
use zenpod_app::application::snapshot::Snapshot;

use super::{blocking, pick_folder, zen, CmdResult};
use crate::state::AppState;

#[tauri::command(async)]
pub fn scan_devices(state: State<AppState>) -> Vec<DeviceEntry> {
    state.zen.devices.scan()
}

/// 「フォルダを指定して開く」: Rust で開いたダイアログでフォルダを選び、open_device で開いてよい場所にする。
/// 戻り値は選んだフォルダ (やめたら None)
#[tauri::command]
pub async fn pick_device_folder(window: Window, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let zen = state.zen.clone();
    blocking(move || {
        let dir = pick_folder(&window, t!("iPod のドライブ / フォルダ"))?;
        zen.devices.allow_folder(dir.clone());
        Some(dir.to_string_lossy().into_owned())
    })
    .await
}

/// DB の読み込みに数秒かかることがあるので、画面を止めないよう別スレッドで開く。
/// `mode` を省くと、Rockbox が入っている iPod は Rockbox モード、それ以外は iPod OS モードで開く。
/// 開けるのは、つながっている iPod と pick_device_folder で選んだフォルダだけ
#[tauri::command]
pub async fn open_device(app: AppHandle, root: String, mode: Option<Mode>) -> CmdResult<Snapshot> {
    let zen = zen(&app);
    Ok(blocking(move || zen.devices.open_requested(&PathBuf::from(root), mode)).await??)
}

#[tauri::command]
pub fn close_device(state: State<AppState>) {
    state.zen.devices.close();
}

#[tauri::command(async)]
pub fn snapshot(state: State<AppState>) -> CmdResult<Option<Snapshot>> {
    Ok(state.zen.session.snapshot())
}

/// 開いている iPod がまだ接続されているか (2.5 秒ごとに呼ばれる。iPod を使っている処理があっても待たない)。
/// iPod のディスクを見るので、画面と別のスレッドで (ディスクが眠っていると待たされることがある)
#[tauri::command(async)]
pub fn device_present(state: State<AppState>) -> bool {
    state.zen.devices.present()
}

/// 戻り値は OS に取り出してもらったか (false はフォルダで開いた iPod を閉じただけ)
#[tauri::command]
pub async fn eject_device(state: State<'_, AppState>) -> CmdResult<bool> {
    let zen = state.zen.clone();
    Ok(blocking(move || zen.devices.eject()).await??)
}

#[tauri::command(async)]
pub fn set_firewire_id(state: State<AppState>, hex: String) -> CmdResult<Snapshot> {
    Ok(state.zen.devices.set_firewire_id(&hex)?)
}

#[tauri::command(async)]
pub fn rename_ipod(state: State<AppState>, name: String) -> CmdResult<Snapshot> {
    Ok(state.zen.devices.rename(&name)?)
}

/// iPod が USB につながっているか (画面の起動時に、イベントを受ける前の状態を知るため)
#[tauri::command]
pub fn usb_ipod_connected(state: State<AppState>) -> bool {
    state.zen.devices.usb_connected()
}

/// USB で認識したのにドライブとして見えないときの理由 ("fsck" / "denied" / "none"。macOS 以外は null)。
/// 別のプログラム (ps・mount) を動かすので画面と別のスレッドで
#[tauri::command(async)]
pub fn drive_wait_reason(state: State<AppState>) -> Option<&'static str> {
    state.zen.devices.drive_wait_reason()
}
