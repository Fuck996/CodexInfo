use super::{
    get_settings, position_hover_component, read_stored_settings, refresh_tray_menu,
    update_dock_window, write_stored_settings, AppSettings, AppState,
};
use serde::Serialize;
use tauri::{window::Monitor, AppHandle, Emitter, Manager};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MonitorOption {
    pub id: String,
    pub label: String,
}

pub(super) fn target_monitor(app: &AppHandle) -> Option<Monitor> {
    let state = app.state::<AppState>();
    let selected = state.selected_monitor.lock().ok()?.clone();
    if let Some(name) = selected {
        if let Some(monitor) = app
            .available_monitors()
            .ok()?
            .into_iter()
            .find(|monitor| monitor.name() == Some(&name))
        {
            return Some(monitor);
        }
    }
    // 显示器断开时暂放主屏，保留选择，重新连接后自动恢复。
    app.primary_monitor().ok().flatten()
}

#[tauri::command]
pub(super) fn get_monitors(app: AppHandle) -> Result<Vec<MonitorOption>, String> {
    let primary = app.primary_monitor().map_err(|error| error.to_string())?;
    Ok(app
        .available_monitors()
        .map_err(|error| error.to_string())?
        .iter()
        .enumerate()
        .filter_map(|(index, monitor)| {
            let name = monitor.name()?.clone();
            let is_primary = primary
                .as_ref()
                .is_some_and(|primary| primary.name() == monitor.name());
            Some(MonitorOption {
                id: name.clone(),
                label: format!(
                    "显示器 {}{} · {} × {} · {}% · {}",
                    index + 1,
                    if is_primary { "（主屏）" } else { "" },
                    monitor.size().width,
                    monitor.size().height,
                    (monitor.scale_factor() * 100.0).round(),
                    name.trim_start_matches("\\\\.\\")
                ),
            })
        })
        .collect())
}

#[tauri::command]
pub(super) fn set_selected_monitor(
    app: AppHandle,
    monitor_id: Option<String>,
) -> Result<AppSettings, String> {
    if let Some(id) = &monitor_id {
        if !app
            .available_monitors()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|monitor| monitor.name() == Some(id))
        {
            return Err("所选显示器已断开，请重新选择".into());
        }
    }
    let mut settings = read_stored_settings(&app)?;
    settings.selected_monitor = monitor_id.clone();
    write_stored_settings(&app, &settings)?;
    let state = app.state::<AppState>();
    *state
        .selected_monitor
        .lock()
        .map_err(|error| error.to_string())? = monitor_id;
    update_dock_window(&app, &state, true);
    if let Some(window) = app.get_webview_window("main") {
        position_hover_component(&app, &window);
    }
    refresh_tray_menu(&app);
    let result = get_settings(app.clone())?;
    app.emit("usage-display-changed", &result)
        .map_err(|error| error.to_string())?;
    Ok(result)
}
