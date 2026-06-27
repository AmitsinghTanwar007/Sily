//! `sily update` — self-update by fetching the latest GitHub release and
//! replacing the running binary in place. Shells out to `curl`/`tar` (always
//! present on the supported platforms) to avoid pulling in an HTTP/TLS stack.

use std::path::Path;
use std::process::Command;

const REPO: &str = "AmitsinghTanwar007/Sily";

/// The release asset for the current platform, e.g. `sily-linux-x86_64.tar.gz`.
fn asset_name() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        other => return Err(format!("unsupported OS for self-update: {other}")),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        other => return Err(format!("unsupported architecture for self-update: {other}")),
    };
    Ok(format!("sily-{os}-{arch}.tar.gz"))
}

/// Best-effort: the latest release tag via the GitHub API (e.g. "v0.3.0").
fn latest_version() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("tag_name")?.as_str().map(str::to_string)
}

fn perm_hint(e: std::io::Error, dir: &Path) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            format!("no permission to write {} — try:  sudo sily update", dir.display())
        }
        _ => e.to_string(),
    }
}

pub fn run() -> Result<(), String> {
    let asset = asset_name()?;
    let current = env!("CARGO_PKG_VERSION");
    println!("sily: installed version v{current}");

    match latest_version() {
        Some(latest) => {
            println!("sily: latest release {latest}");
            if latest.trim_start_matches('v') == current {
                println!("sily: already up to date.");
                return Ok(());
            }
        }
        None => println!("sily: couldn't check latest version; fetching newest anyway…"),
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let dir = exe.parent().ok_or("cannot locate install directory")?;

    let tmp = std::env::temp_dir().join(format!("sily-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let tarball = tmp.join(&asset);
    let url = format!("https://github.com/{REPO}/releases/latest/download/{asset}");

    println!("sily: downloading {asset}…");
    let dl = Command::new("curl")
        .args(["-fsSL", &url, "-o"])
        .arg(&tarball)
        .status()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !dl.success() {
        return Err("download failed".to_string());
    }

    let untar = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("tar failed: {e}"))?;
    if !untar.success() {
        return Err("failed to extract archive".to_string());
    }

    // Stage in the target directory so the final swap is an atomic same-filesystem
    // rename (which works even while this binary is running on Unix).
    let new_bin = tmp.join("sily");
    let staged = dir.join(".sily.update");
    std::fs::copy(&new_bin, &staged).map_err(|e| perm_hint(e, dir))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&staged, &exe).map_err(|e| perm_hint(e, dir))?;
    let _ = std::fs::remove_dir_all(&tmp);

    println!("sily: updated successfully → {}", exe.display());
    Ok(())
}
