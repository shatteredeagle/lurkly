fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "auth_start",
            "auth_poll",
            "auth_status",
            "logout",
            "live_follows",
            "check_update",
            "open_release",
        ]),
    ))
    .expect("failed to run tauri-build");
}
