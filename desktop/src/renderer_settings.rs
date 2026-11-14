use tauri::WebviewWindow;
use zephium_core::linux_graphics::RendererPreference;
use zephium_ipc::linux_graphics::GpuReport;

use crate::{authorize, CallerPolicy};

#[tauri::command]
#[specta::specta]
pub(crate) async fn gpu_report(caller: WebviewWindow) -> Result<GpuReport, String> {
    if !authorize(&caller, CallerPolicy::Main, "gpu_report") {
        return Err("GPU report is restricted to the main interface".into());
    }
    #[cfg(target_os = "linux")]
    return tauri::async_runtime::spawn_blocking(crate::linux_graphics::report)
        .await
        .map_err(|_| "GPU report task failed".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    Err("GPU renderer selection is available on Linux only".into())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn renderer_preference_set(
    caller: WebviewWindow,
    preference: RendererPreference,
) -> Result<GpuReport, String> {
    if !authorize(&caller, CallerPolicy::Main, "renderer_preference_set") {
        return Err("Renderer preferences are restricted to the main interface".into());
    }
    #[cfg(target_os = "linux")]
    return tauri::async_runtime::spawn_blocking(move || {
        crate::linux_graphics::set_preference(preference)
    })
    .await
    .map_err(|_| "Renderer preference task failed".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = preference;
        Err("GPU renderer selection is available on Linux only".into())
    }
}
