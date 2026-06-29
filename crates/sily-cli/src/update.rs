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

/// Best-effort: the latest release tag via GitHub's redirect from
/// `/releases/latest` to `/releases/tag/vX.Y.Z`. This avoids the REST API's
/// unauthenticated rate limit.
fn latest_version_via_redirect() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsSIL",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            &format!("https://github.com/{REPO}/releases/latest"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_tag_from_release_url(std::str::from_utf8(&out.stdout).ok()?.trim())
}

/// Fallback: the latest release tag via the GitHub API (e.g. "v0.3.0").
fn latest_version_via_api() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            &format!("User-Agent: sily/{}", env!("CARGO_PKG_VERSION")),
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

fn latest_version() -> Option<String> {
    latest_version_via_redirect().or_else(latest_version_via_api)
}

fn parse_tag_from_release_url(url: &str) -> Option<String> {
    url.rsplit("/tag/").next().and_then(|s| {
        let tag = s.trim();
        (!tag.is_empty() && tag.starts_with('v')).then(|| tag.to_string())
    })
}

fn download_url(asset: &str, latest: Option<&str>) -> String {
    match latest {
        Some(tag) => format!("https://github.com/{REPO}/releases/download/{tag}/{asset}"),
        None => format!("https://github.com/{REPO}/releases/latest/download/{asset}"),
    }
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

    let latest = latest_version();
    match latest.as_deref() {
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
    let url = download_url(&asset, latest.as_deref());

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

#[cfg(test)]
mod tests {
    use super::{download_url, parse_tag_from_release_url};

    #[test]
    fn parses_tag_from_release_redirect() {
        let tag = parse_tag_from_release_url(
            "https://github.com/AmitsinghTanwar007/Sily/releases/tag/v0.22.1",
        );
        assert_eq!(tag.as_deref(), Some("v0.22.1"));
    }

    #[test]
    fn tag_specific_download_url_is_stable() {
        let url = download_url("sily-macos-arm64.tar.gz", Some("v0.22.1"));
        assert_eq!(
            url,
            "https://github.com/AmitsinghTanwar007/Sily/releases/download/v0.22.1/sily-macos-arm64.tar.gz"
        );
    }

    #[test]
    fn latest_redirect_download_url_is_fallback_only() {
        let url = download_url("sily-linux-x86_64.tar.gz", None);
        assert_eq!(
            url,
            "https://github.com/AmitsinghTanwar007/Sily/releases/latest/download/sily-linux-x86_64.tar.gz"
        );
    }
}
