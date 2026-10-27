fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["open_login", "has_token", "fetch_subs"]),
        ),
    )
    .expect("failed to run tauri-build");
}
