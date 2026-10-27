fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "auth_start",
            "auth_poll",
            "auth_status",
            "logout",
            "get_live",
            "refresh_now",
            "test_alert",
            "take_pending_open",
            "get_alerts",
            "set_alerts_global",
            "snooze_alerts",
            "set_channel_alert",
            "set_autostart",
            "followed_channels",
            "check_update",
            "open_release",
        ]),
    ))
    .expect("failed to run tauri-build");
}
