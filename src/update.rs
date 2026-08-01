//! Auto-update: check GitHub Releases for a newer version, download the
//! platform package, replace the running app and relaunch (Zed-style).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

pub const REPO_OWNER: &str = "csic21";
pub const REPO_NAME: &str = "zstock";

const RELEASE_API: &str = "https://api.github.com/repos/csic21/zstock/releases/latest";
const APP_BUNDLE_NAME: &str = "Stock Analysis.app";
const BINARY_NAME: &str = "stock";

/// UI state of the auto-updater.
#[derive(Debug, Clone, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Available(UpdateInfo),
    Downloading(String),
    UpToDate,
    Error(String),
}

/// A newer release that can be installed on this platform.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub asset_url: String,
    pub release_url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

fn user_agent() -> String {
    format!(
        "stock-updater/{} (+github.com/{}/{})",
        env!("CARGO_PKG_VERSION"),
        REPO_OWNER,
        REPO_NAME
    )
}

fn platform_suffix() -> &'static str {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "macos-arm64.zip"
    } else if cfg!(target_os = "macos") {
        "macos-x64.zip"
    } else if cfg!(target_os = "windows") {
        "windows-x64.zip"
    } else {
        "unsupported.zip"
    }
}

fn parse_version(tag: &str) -> Result<semver::Version, String> {
    semver::Version::parse(tag.trim_start_matches('v'))
        .map_err(|e| format!("版本号解析失败（{tag}）：{e}"))
}

/// Query the latest GitHub release. Returns `Some` when a newer version
/// exists for the current platform, `None` when already up to date.
pub fn check_latest() -> Result<Option<UpdateInfo>, String> {
    let resp = ureq::get(RELEASE_API)
        .set("User-Agent", &user_agent())
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("检查更新失败：{e}"))?;
    let release: Release = resp
        .into_json()
        .map_err(|e| format!("解析更新信息失败：{e}"))?;

    let latest = parse_version(&release.tag_name)?;
    let current = parse_version(env!("CARGO_PKG_VERSION"))?;
    if latest <= current {
        return Ok(None);
    }

    let suffix = platform_suffix();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(suffix))
        .ok_or_else(|| format!("该版本未提供当前平台安装包（{suffix}）"))?;

    Ok(Some(UpdateInfo {
        version: latest.to_string(),
        notes: release.body.unwrap_or_default(),
        asset_url: asset.browser_download_url.clone(),
        release_url: release.html_url.clone(),
    }))
}

/// Download the platform package, replace the running app and relaunch.
/// On success this process exits.
pub fn download_and_install(info: &UpdateInfo) -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("stock-update-{}", std::process::id()));
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|e| format!("清理临时目录失败：{e}"))?;
    }
    fs::create_dir_all(&work).map_err(|e| format!("创建临时目录失败：{e}"))?;

    let zip_path = work.join("update.zip");
    let resp = ureq::get(&info.asset_url)
        .set("User-Agent", &user_agent())
        .call()
        .map_err(|e| format!("下载更新包失败：{e}"))?;
    let mut reader = resp.into_reader();
    let mut file = fs::File::create(&zip_path).map_err(|e| format!("写入更新包失败：{e}"))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| format!("下载中断：{e}"))?;
    drop(file);

    let extracted = work.join("extracted");
    fs::create_dir_all(&extracted).map_err(|e| format!("创建解压目录失败：{e}"))?;
    let file = fs::File::open(&zip_path).map_err(|e| format!("打开更新包失败：{e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("更新包损坏：{e}"))?;
    archive
        .extract(&extracted)
        .map_err(|e| format!("解压更新包失败：{e}"))?;

    #[cfg(target_os = "macos")]
    install_macos(&extracted)?;
    #[cfg(target_os = "windows")]
    install_windows(&extracted)?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Err("当前平台暂不支持自动更新".to_string());

    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos(extracted: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位当前程序失败：{e}"))?;

    if let Some(bundle) = bundle_dir(&exe) {
        // Running inside a .app bundle: swap the whole bundle.
        let new_bundle = extracted.join(APP_BUNDLE_NAME);
        if !new_bundle.is_dir() {
            return Err(format!("更新包中缺少 {APP_BUNDLE_NAME}"));
        }
        let parent = bundle.parent().ok_or_else(|| "应用目录无效".to_string())?;
        let file_name = bundle
            .file_name()
            .ok_or_else(|| "应用目录无效".to_string())?;
        let backup = parent.join(format!("{}.old-update", file_name.to_string_lossy()));

        let _ = fs::remove_dir_all(&backup);
        fs::rename(&bundle, &backup).map_err(|e| format!("无法移动旧应用（目录只读？）：{e}"))?;
        if let Err(e) = fs::rename(&new_bundle, &bundle) {
            // Fallback for cross-volume moves.
            if let Err(copy_err) = copy_dir_all(&new_bundle, &bundle) {
                let _ = fs::rename(&backup, &bundle); // roll back
                return Err(format!("替换应用失败：{e} / {copy_err}"));
            }
        }
        // Re-sign ad-hoc so the bundle stays consistent after the swap.
        let _ = Command::new("codesign")
            .args(["--force", "-s", "-"])
            .arg(&bundle)
            .output();
        let _ = fs::remove_dir_all(&backup);

        Command::new("open")
            .arg(&bundle)
            .spawn()
            .map_err(|e| format!("启动新版本失败：{e}"))?;
    } else {
        // Standalone binary (e.g. `cargo run`): replace in place.
        replace_binary(&exe, &extracted.join(BINARY_NAME))?;
        Command::new(&exe)
            .spawn()
            .map_err(|e| format!("启动新版本失败：{e}"))?;
    }

    // Give the new process a moment to start, then quit.
    std::thread::sleep(std::time::Duration::from_millis(800));
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
fn bundle_dir(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.extension().map_or(false, |e| e == "app"))
        .map(Path::to_path_buf)
}

#[cfg(target_os = "macos")]
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else if ty.is_symlink() {
            let target = fs::read_link(entry.path())?;
            std::os::unix::fs::symlink(target, &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_windows(extracted: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位当前程序失败：{e}"))?;
    let new_exe = find_named_file(extracted, "stock.exe")
        .ok_or_else(|| "更新包中缺少 stock.exe".to_string())?;
    replace_binary(&exe, &new_exe)?;
    Command::new(&exe)
        .spawn()
        .map_err(|e| format!("启动新版本失败：{e}"))?;

    std::thread::sleep(std::time::Duration::from_millis(800));
    std::process::exit(0);
}

/// Replace `current` with `new`, keeping a `.old` copy that is removed when
/// possible (Windows keeps the running image locked, so cleanup may be deferred).
fn replace_binary(current: &Path, new: &Path) -> Result<(), String> {
    let backup = current.with_extension("exe.old");
    let _ = fs::remove_file(&backup);
    fs::rename(current, &backup).map_err(|e| format!("无法重命名当前程序：{e}"))?;
    if let Err(e) = fs::copy(new, current) {
        let _ = fs::rename(&backup, current);
        return Err(format!("写入新程序失败：{e}"));
    }
    let _ = fs::remove_file(&backup);
    Ok(())
}

#[cfg(target_os = "windows")]
fn find_named_file(root: &Path, name: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
        if depth > 4 {
            return None;
        }
        for entry in fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path, name, depth + 1) {
                    return Some(found);
                }
            } else if path.file_name().map_or(false, |f| f == name) {
                return Some(path);
            }
        }
        None
    }
    walk(root, name, 0)
}
