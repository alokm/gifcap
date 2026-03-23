pub mod capture;
pub mod commands;
pub mod compression;
pub mod platform;

use commands::capture::CaptureState;
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(CaptureState::default())
        .invoke_handler(tauri::generate_handler![
            commands::capture::start_recording,
            commands::capture::stop_recording,
            commands::capture::pause_recording,
            commands::capture::resume_recording,
            commands::compression::encode_preset,
            commands::compression::start_preset_encode,
            commands::compression::save_gif,
            commands::compression::discard_recording,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::Manager;
                if let Some(win) = app.get_webview_window("capture") {
                    // Start click-through; watcher enables events over interactive zones.
                    let _ = win.set_ignore_cursor_events(true);
                    platform::macos::start_click_through_watcher(win);
                }

                // Trigger the TCC screen-recording permission prompt on first launch
                // so the user is asked immediately rather than when they click Record.
                std::thread::spawn(|| {
                    use screencapturekit::shareable_content::SCShareableContent;
                    if let Err(e) = SCShareableContent::get() {
                        log::info!("Screen recording permission not yet granted (will prompt via TCC): {:?}", e);
                    } else {
                        log::info!("Screen recording permission already granted.");
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
