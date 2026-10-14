//! Exposes the product version to the binary as `BUDDY_VERSION`, and on
//! Windows embeds the app icon.
//!
//! The release workflow advances only the repository's `package.json`, so
//! the native build reads its version from there and falls back to the
//! crate version when the file is not available (a standalone checkout of
//! `rust/`).

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let desktop_manifest = Path::new(&manifest_dir).join("../../desktop-package.json");
    println!("cargo:rerun-if-changed={}", desktop_manifest.display());
    let desktop: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&desktop_manifest).expect("read desktop-package.json"),
    )
    .expect("parse desktop-package.json");
    let product_name = desktop["productName"]
        .as_str()
        .expect("desktop productName");
    let binary_name = desktop["binaryName"].as_str().expect("desktop binaryName");
    let identifier = desktop["identifier"].as_str().expect("desktop identifier");
    // Wayland resolves the app icon against <app_id>.desktop. cargo-packager
    // names that entry after the binary; other platforms use the bundle ID.
    let app_id = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        binary_name
    } else {
        identifier
    };
    println!("cargo:rustc-env=BUDDY_PRODUCT_NAME={product_name}");
    println!("cargo:rustc-env=BUDDY_BINARY_NAME={binary_name}");
    println!("cargo:rustc-env=BUDDY_APP_ID={app_id}");
    embed_windows_icon(Path::new(&manifest_dir));
    let package_json = Path::new(&manifest_dir).join("../../../package.json");
    println!("cargo:rerun-if-changed={}", package_json.display());
    let version = std::fs::read_to_string(&package_json)
        .ok()
        .and_then(|body| package_version(&body))
        .unwrap_or_else(|| {
            std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION")
        });
    println!("cargo:rustc-env=BUDDY_VERSION={version}");
}

fn package_version(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let version = parsed.get("version")?.as_str()?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// GPUI's Windows backend takes the window-class icon (taskbar, Alt+Tab,
/// title bar) from icon resource 1 of the running executable, and Explorer
/// shows the first icon group. Embed Electron's `public/icon.ico` as that
/// resource, so the two apps share one icon file.
fn embed_windows_icon(manifest_dir: &Path) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = manifest_dir.join("../../../public/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let Ok(icon) = icon.canonicalize() else {
        println!(
            "cargo:warning=public/icon.ico not found; buddy.exe gets the default Windows icon"
        );
        return;
    };
    compile_icon_resource(&icon);
}

#[cfg(windows)]
fn compile_icon_resource(icon: &Path) {
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let rc = Path::new(&out_dir).join("buddy-icon.rc");
    // rc.exe takes forward slashes; `canonicalize` adds a `\\?\` prefix it rejects.
    let path = icon.display().to_string();
    let path = path.trim_start_matches(r"\\?\").replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).expect("write buddy-icon.rc");
    embed_resource::compile_for(&rc, ["buddy"], embed_resource::NONE)
        .manifest_required()
        .expect("compile the buddy.exe icon resource (needs rc.exe from the Windows SDK)");
}

// Cross-compiling to Windows from another host: embed-resource is a
// Windows-host build dependency, so the icon is skipped.
#[cfg(not(windows))]
fn compile_icon_resource(_icon: &Path) {
    println!("cargo:warning=building for Windows on a non-Windows host; buddy.exe has no icon");
}
