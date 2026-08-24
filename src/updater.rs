use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{ACCEPT, CONTENT_LENGTH};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/oshtz/cedar/releases/latest";
const UPDATE_USER_AGENT: &str = "Cedar native updater";
const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct UpdateInfo {
    pub(crate) version: String,
    pub(crate) notes: String,
    pub(crate) published_at: Option<String>,
    pub(crate) asset_name: String,
    pub(crate) download_url: String,
    pub(crate) sha256: String,
    pub(crate) size: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct DownloadedUpdate {
    pub(crate) info: UpdateInfo,
    pub(crate) path: PathBuf,
    pub(crate) verification: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading(UpdateInfo),
    Ready(DownloadedUpdate),
    Installing,
    Error(String),
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: String,
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

pub(crate) fn supported() -> bool {
    cfg!(windows) || cfg!(target_os = "macos")
}

pub(crate) async fn check_for_update() -> Result<Option<UpdateInfo>> {
    if !supported() {
        bail!("Automatic updates are only available on Windows and macOS.");
    }

    let client = update_client()?;
    let release_bytes = request_bytes(
        &client,
        LATEST_RELEASE_URL,
        MAX_METADATA_BYTES,
        "application/vnd.github+json",
    )
    .await?;
    let release: GithubRelease = serde_json::from_slice(&release_bytes)
        .context("GitHub returned invalid release metadata")?;
    update_for_release(env!("CARGO_PKG_VERSION"), &release, &client).await
}

async fn update_for_release(
    current_version: &str,
    release: &GithubRelease,
    client: &reqwest::Client,
) -> Result<Option<UpdateInfo>> {
    let version = parse_release_version(&release.tag_name)?;
    let current =
        Version::parse(current_version).context("Cedar has an invalid package version")?;
    if version <= current {
        return Ok(None);
    }

    let version_text = version.to_string();
    let asset_name = package_asset_name()?.to_owned();
    let checksum_name = checksum_asset_name()?;
    let asset = find_asset(release, &asset_name)?;
    if asset.size.is_some_and(|size| size > MAX_PACKAGE_BYTES) {
        bail!("The update package exceeds Cedar's 512 MiB limit.");
    }
    validate_download_url(&asset.browser_download_url)?;

    let checksum_asset = find_asset(release, checksum_name)?;
    validate_download_url(&checksum_asset.browser_download_url)?;
    let checksum_bytes = request_bytes(
        client,
        &checksum_asset.browser_download_url,
        MAX_CHECKSUM_BYTES,
        "text/plain",
    )
    .await?;
    let checksum_text = String::from_utf8(checksum_bytes)
        .context("The release checksum manifest is not valid UTF-8")?;
    let sha256 = checksum_for_asset(&checksum_text, &asset_name)?;

    Ok(Some(UpdateInfo {
        version: version_text,
        notes: release.body.trim().to_owned(),
        published_at: release.published_at.clone(),
        asset_name,
        download_url: asset.browser_download_url.clone(),
        sha256,
        size: asset.size,
    }))
}

pub(crate) async fn download_update(info: UpdateInfo) -> Result<DownloadedUpdate> {
    validate_download_url(&info.download_url)?;
    validate_sha256(&info.sha256)?;
    if info.size.is_some_and(|size| size > MAX_PACKAGE_BYTES) {
        bail!("The update package exceeds Cedar's 512 MiB limit.");
    }

    let update_dir = update_dir()?;
    fs::create_dir_all(&update_dir).context("Could not create Cedar's update directory")?;
    let destination = update_dir.join(&info.asset_name);
    let partial = update_dir.join(format!("{}.part", info.asset_name));
    let _ = fs::remove_file(&partial);

    let client = update_client()?;
    let mut response = client
        .get(&info.download_url)
        .header(ACCEPT, "application/octet-stream")
        .send()
        .await
        .context("The update download failed")?
        .error_for_status()
        .context("The update download returned an error")?;
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > MAX_PACKAGE_BYTES)
    {
        bail!("The update package exceeds Cedar's 512 MiB limit.");
    }

    let download_result = async {
        let mut file =
            fs::File::create(&partial).context("Could not save the downloaded update")?;
        let mut hasher = Sha256::new();
        let mut downloaded = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .context("Could not read the update download")?
        {
            downloaded += chunk.len() as u64;
            if downloaded > MAX_PACKAGE_BYTES {
                bail!("The update package exceeds Cedar's 512 MiB limit.");
            }
            file.write_all(&chunk)
                .context("Could not save the update download")?;
            hasher.update(&chunk);
        }
        file.flush()
            .context("Could not finalize the update download")?;
        if downloaded == 0 {
            bail!("The update download was empty.");
        }
        Ok::<_, anyhow::Error>((downloaded, format!("{:x}", hasher.finalize())))
    }
    .await;
    let (downloaded, actual_hash) = match download_result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
    };
    if let Some(expected_size) = info.size
        && downloaded != expected_size
    {
        let _ = fs::remove_file(&partial);
        bail!(
            "The update package size did not match GitHub metadata (expected {expected_size}, received {downloaded})."
        );
    }
    if actual_hash != info.sha256 {
        let _ = fs::remove_file(&partial);
        bail!("The downloaded update failed SHA-256 verification.");
    }

    if destination.exists() {
        remove_update_path(&destination)?;
    }
    fs::rename(&partial, &destination).context("Could not finalize the downloaded update")?;

    #[cfg(target_os = "macos")]
    let path = extract_macos_app(&destination)?;
    #[cfg(not(target_os = "macos"))]
    let path = destination;

    let verification = verify_platform_signature(&path)?;
    Ok(DownloadedUpdate {
        info,
        path,
        verification,
    })
}

async fn request_bytes(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
    accept: &str,
) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .header(ACCEPT, accept)
        .send()
        .await
        .with_context(|| format!("Update request failed for {url}"))?
        .error_for_status()
        .with_context(|| format!("Update request returned an error for {url}"))?;
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > max_bytes)
    {
        bail!("The update response exceeded Cedar's configured size limit.");
    }
    let bytes = response
        .bytes()
        .await
        .context("Could not read the update response")?;
    if bytes.is_empty() {
        bail!("The update response was empty.");
    }
    if bytes.len() as u64 > max_bytes {
        bail!("The update response exceeded Cedar's configured size limit.");
    }
    Ok(bytes.to_vec())
}

fn update_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .user_agent(UPDATE_USER_AGENT)
        .build()
        .context("Could not initialize Cedar's update client")
}

fn parse_release_version(tag: &str) -> Result<Version> {
    Version::parse(tag.trim().trim_start_matches(['v', 'V']))
        .with_context(|| format!("GitHub release tag {tag:?} is not semantic versioning"))
}

fn package_asset_name() -> Result<&'static str> {
    if cfg!(windows) {
        Ok("Cedar.exe")
    } else if cfg!(target_os = "macos") {
        Ok("Cedar_macos.app.zip")
    } else {
        bail!("Automatic updates are unsupported on this platform.")
    }
}

fn checksum_asset_name() -> Result<&'static str> {
    if cfg!(windows) {
        Ok("SHA256SUMS-windows.txt")
    } else if cfg!(target_os = "macos") {
        Ok("SHA256SUMS-macos.txt")
    } else {
        bail!("Automatic updates are unsupported on this platform.")
    }
}

fn find_asset<'a>(release: &'a GithubRelease, name: &str) -> Result<&'a GithubAsset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| anyhow!("Release {} is missing {name}.", release.tag_name))
}

fn checksum_for_asset(manifest: &str, asset_name: &str) -> Result<String> {
    for line in manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        if name.trim_start_matches('*') == asset_name {
            let hash = hash.to_ascii_lowercase();
            validate_sha256(&hash)?;
            return Ok(hash);
        }
    }
    bail!("The checksum manifest does not contain {asset_name}.")
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Ok(())
    } else {
        bail!("The release manifest contains an invalid SHA-256 checksum.")
    }
}

fn validate_download_url(url: &str) -> Result<()> {
    let rest = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| anyhow!("Update assets must use HTTPS GitHub release URLs."))?;
    let path = rest.to_ascii_lowercase();
    if !path.starts_with("oshtz/cedar/releases/download/")
        || path.contains('@')
        || path.starts_with('/')
    {
        bail!("Update assets must come from the oshtz/cedar GitHub release feed.");
    }
    Ok(())
}

fn update_dir() -> Result<PathBuf> {
    dirs::data_local_dir()
        .map(|path| path.join("Cedar").join("updates"))
        .ok_or_else(|| anyhow!("Cedar's local data directory is unavailable."))
}

fn remove_update_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn extract_macos_app(archive: &Path) -> Result<PathBuf> {
    let parent = archive
        .parent()
        .ok_or_else(|| anyhow!("The macOS update archive has no parent directory."))?;
    let app_path = parent.join("Cedar.app");
    if app_path.exists() {
        fs::remove_dir_all(&app_path)?;
    }
    let status = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(parent)
        .status()
        .context("Could not extract the macOS update")?;
    if !status.success() || !app_path.is_dir() {
        bail!("The macOS update did not contain Cedar.app.");
    }
    let _ = fs::remove_file(archive);
    Ok(app_path)
}

fn verify_platform_signature(path: &Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let codesign = Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict", "--verbose=2"])
            .arg(path)
            .output()
            .context("Could not verify the macOS update signature")?;
        if !codesign.status.success() {
            bail!(
                "The macOS update has an invalid code signature: {}",
                String::from_utf8_lossy(&codesign.stderr).trim()
            );
        }
        let gatekeeper = Command::new("/usr/sbin/spctl")
            .args(["--assess", "--type", "execute", "--verbose=2"])
            .arg(path)
            .output()
            .context("Could not ask Gatekeeper to verify the macOS update")?;
        if !gatekeeper.status.success() {
            bail!(
                "Gatekeeper rejected the macOS update: {}",
                String::from_utf8_lossy(&gatekeeper.stderr).trim()
            );
        }
        Ok("Signed and accepted by Gatekeeper".into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::{
            Foundation::{
                HWND, TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN, TRUST_E_SUBJECT_FORM_UNKNOWN,
            },
            Security::WinTrust::{
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
                WTD_SAFER_FLAG, WTD_STATEACTION_IGNORE, WTD_UI_NONE, WTD_UICONTEXT_EXECUTE,
                WinVerifyTrust,
            },
        };
        use windows::core::PCWSTR;

        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(wide_path.as_ptr()),
            ..Default::default()
        };
        let mut trust_data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &mut file_info,
            },
            dwStateAction: WTD_STATEACTION_IGNORE,
            dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_SAFER_FLAG,
            dwUIContext: WTD_UICONTEXT_EXECUTE,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = unsafe {
            WinVerifyTrust(
                HWND::default(),
                &mut action,
                (&mut trust_data as *mut WINTRUST_DATA).cast(),
            )
        };
        match status {
            0 => Ok("SHA-256 and publisher signature verified".into()),
            status
                if [
                    TRUST_E_NOSIGNATURE.0,
                    TRUST_E_PROVIDER_UNKNOWN.0,
                    TRUST_E_SUBJECT_FORM_UNKNOWN.0,
                ]
                .contains(&status) =>
            {
                Ok("SHA-256 verified · unsigned publisher".into())
            }
            status => bail!(
                "The Windows update has an invalid publisher signature (WinVerifyTrust status 0x{:08x}).",
                status as u32
            ),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        Ok("SHA-256 verified".into())
    }
}

pub(crate) fn stage_install(download: &DownloadedUpdate) -> Result<()> {
    let update_root = fs::canonicalize(update_dir()?)
        .context("Cedar's update directory could not be resolved")?;
    let source =
        fs::canonicalize(&download.path).context("The downloaded update could not be resolved")?;
    if !source.starts_with(&update_root) {
        bail!("The update source must remain inside Cedar's update directory.");
    }
    let current = std::env::current_exe().context("Cedar's executable could not be resolved")?;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("The system clock is unavailable")?
            .as_nanos()
    );
    let health_path = update_root.join(format!("update-health-{nonce}.txt"));
    let _ = fs::remove_file(&health_path);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        if !source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            bail!("Windows updates must be executable files.");
        }
        let target = windows_install_target(&current);
        if target != current && target.exists() {
            bail!(
                "Cedar.exe already exists beside this legacy executable. Move or remove it before installing the update."
            );
        }
        let backup = current.with_extension("exe.previous");
        let log = update_root.join("install.log");
        let recovery_error = update_root.join("install-error.txt");
        let script_path = update_root.join("apply-update.ps1");
        fs::write(&script_path, WINDOWS_INSTALL_SCRIPT)
            .context("Could not prepare Cedar's update installer")?;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
            ])
            .arg(&script_path)
            .env("CEDAR_UPDATE_PID", std::process::id().to_string())
            .env("CEDAR_UPDATE_SOURCE", source)
            .env("CEDAR_UPDATE_CURRENT", current)
            .env("CEDAR_UPDATE_TARGET", target)
            .env("CEDAR_UPDATE_BACKUP", backup)
            .env("CEDAR_UPDATE_LOG", log)
            .env("CEDAR_UPDATE_RECOVERY_ERROR", recovery_error)
            .env("CEDAR_UPDATE_SCRIPT", &script_path)
            .env("CEDAR_UPDATE_HEALTH_PATH", &health_path)
            .env("CEDAR_UPDATE_HEALTH_NONCE", &nonce)
            .creation_flags(0x0800_0000);
        command
            .spawn()
            .context("Could not start Cedar's update installer")?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let source_app = source
            .ancestors()
            .find(|path| path.extension().is_some_and(|value| value == "app"))
            .ok_or_else(|| anyhow!("The macOS update did not contain an app bundle."))?;
        let target_app = current
            .ancestors()
            .find(|path| path.extension().is_some_and(|value| value == "app"))
            .ok_or_else(|| anyhow!("Cedar's current app bundle could not be resolved."))?;
        let backup = target_app.with_extension("app.previous");
        Command::new("/bin/sh")
            .args([
                "-c",
                MACOS_INSTALL_SCRIPT,
                "cedar-update",
                &std::process::id().to_string(),
                &source_app.to_string_lossy(),
                &target_app.to_string_lossy(),
                &backup.to_string_lossy(),
            ])
            .env("CEDAR_UPDATE_HEALTH_PATH", &health_path)
            .env("CEDAR_UPDATE_HEALTH_NONCE", &nonce)
            .env(
                "CEDAR_UPDATE_RECOVERY_ERROR",
                update_root.join("install-error.txt"),
            )
            .env("CEDAR_UPDATE_LOG", update_root.join("install.log"))
            .spawn()
            .context("Could not start Cedar's update installer")?;
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    bail!("Automatic update installation is unsupported on this platform.")
}

#[cfg(windows)]
fn windows_install_target(current: &Path) -> PathBuf {
    let Some(file_name) = current.file_name().and_then(|name| name.to_str()) else {
        return current.to_owned();
    };
    let lower = file_name.to_ascii_lowercase();
    if lower == "cedar.exe" {
        current.to_owned()
    } else if lower
        .strip_prefix("cedar_")
        .and_then(|name| name.strip_suffix("_windows-x64.exe"))
        .is_some_and(|version| Version::parse(version).is_ok())
    {
        current.with_file_name("Cedar.exe")
    } else {
        current.to_owned()
    }
}

pub(crate) fn stage_executable_name_migration() -> Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        if std::env::var_os("CEDAR_SKIP_NAME_MIGRATION").is_some() {
            return Ok(false);
        }
        let current =
            std::env::current_exe().context("Cedar's executable could not be resolved")?;
        let target = windows_install_target(&current);
        if target == current {
            return Ok(false);
        }
        if target.exists() {
            bail!(
                "Cedar.exe already exists beside this legacy executable, so Cedar kept the current filename."
            );
        }

        let update_root = update_dir()?;
        fs::create_dir_all(&update_root).context("Could not create Cedar's update directory")?;
        let script_path = update_root.join("migrate-executable-name.ps1");
        fs::write(&script_path, WINDOWS_NAME_MIGRATION_SCRIPT)
            .context("Could not prepare Cedar's executable-name migration")?;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
            ])
            .arg(&script_path)
            .env("CEDAR_MIGRATION_PID", std::process::id().to_string())
            .env("CEDAR_MIGRATION_CURRENT", current)
            .env("CEDAR_MIGRATION_TARGET", target)
            .env("CEDAR_MIGRATION_SCRIPT", &script_path)
            .env("CEDAR_MIGRATION_LOG", update_root.join("install.log"))
            .env_remove("CEDAR_UPDATE_HEALTH_PATH")
            .env_remove("CEDAR_UPDATE_HEALTH_NONCE")
            .env_remove("CEDAR_SKIP_NAME_MIGRATION")
            .creation_flags(0x0800_0000);
        command
            .spawn()
            .context("Could not start Cedar's executable-name migration")?;
        Ok(true)
    }

    #[cfg(not(windows))]
    Ok(false)
}

pub(crate) fn complete_update_health() -> Result<()> {
    let (Some(path), Some(nonce)) = (
        std::env::var_os("CEDAR_UPDATE_HEALTH_PATH"),
        std::env::var_os("CEDAR_UPDATE_HEALTH_NONCE"),
    ) else {
        return Ok(());
    };
    let update_root = fs::canonicalize(update_dir()?)?;
    let path = PathBuf::from(path);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("The update health path is invalid."))?;
    if fs::canonicalize(parent)? != update_root {
        bail!("The update health path is outside Cedar's update directory.");
    }
    fs::write(path, nonce.to_string_lossy().as_bytes())
        .context("Could not report a healthy Cedar update")
}

pub(crate) fn take_recovery_error() -> Option<String> {
    let path = update_dir().ok()?.join("install-error.txt");
    let message = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(path);
    let message = message.trim().trim_start_matches('\u{feff}').trim();
    (!message.is_empty()).then(|| message.to_owned())
}

#[cfg(windows)]
const WINDOWS_INSTALL_SCRIPT: &str = r#"param([switch]$Elevated)
$ErrorActionPreference = 'Stop'
$procId = [int]$env:CEDAR_UPDATE_PID
$replacement = $env:CEDAR_UPDATE_SOURCE
$current = $env:CEDAR_UPDATE_CURRENT
$target = $env:CEDAR_UPDATE_TARGET
$backup = $env:CEDAR_UPDATE_BACKUP
$log = $env:CEDAR_UPDATE_LOG
$recoveryError = $env:CEDAR_UPDATE_RECOVERY_ERROR
$scriptPath = $env:CEDAR_UPDATE_SCRIPT
while (Get-Process -Id $procId -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 200 }
for ($attempt = 1; $attempt -le 3; $attempt++) {
  try {
    Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
    if (-not [String]::Equals($current, $target, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $target)) {
      throw 'Cedar.exe already exists beside the legacy executable.'
    }
    Move-Item -LiteralPath $current -Destination $backup -Force
    Move-Item -LiteralPath $replacement -Destination $target -Force
    $updated = Start-Process -FilePath $target -PassThru
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
      if (Test-Path -LiteralPath $env:CEDAR_UPDATE_HEALTH_PATH) {
        $health = Get-Content -LiteralPath $env:CEDAR_UPDATE_HEALTH_PATH -Raw -ErrorAction SilentlyContinue
        if ($health -eq $env:CEDAR_UPDATE_HEALTH_NONCE) {
          Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
          Remove-Item -LiteralPath $env:CEDAR_UPDATE_HEALTH_PATH -Force -ErrorAction SilentlyContinue
          Remove-Item -LiteralPath $recoveryError -Force -ErrorAction SilentlyContinue
          Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
          exit 0
        }
      }
      if ($updated.HasExited) { break }
      Start-Sleep -Milliseconds 200
    }
    if (-not $updated.HasExited) {
      Stop-Process -Id $updated.Id -Force -ErrorAction SilentlyContinue
      $updated.WaitForExit()
    }
    throw 'The updated Cedar process did not report a healthy UI.'
  } catch {
    Add-Content -LiteralPath $log -Value "$(Get-Date -Format s) attempt $attempt failed: $($_.Exception.Message)" -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $backup) {
      Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue
      Move-Item -LiteralPath $backup -Destination $current -Force
      Set-Content -LiteralPath $recoveryError -Value 'The update failed and the previous version was restored.' -Encoding UTF8 -ErrorAction SilentlyContinue
      Remove-Item -LiteralPath $env:CEDAR_UPDATE_HEALTH_PATH -Force -ErrorAction SilentlyContinue
      Remove-Item Env:\CEDAR_UPDATE_HEALTH_PATH -ErrorAction SilentlyContinue
      Remove-Item Env:\CEDAR_UPDATE_HEALTH_NONCE -ErrorAction SilentlyContinue
      Start-Process -FilePath $current
      Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
      exit 1
    }
    if (-not $Elevated) {
      try {
        Start-Process -FilePath 'powershell.exe' -Verb RunAs -WindowStyle Hidden -ArgumentList @(
          '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden',
          '-File', "`"$scriptPath`"", '-Elevated'
        )
        exit 0
      } catch {
        Add-Content -LiteralPath $log -Value "$(Get-Date -Format s) elevation failed: $($_.Exception.Message)" -ErrorAction SilentlyContinue
      }
    }
    Start-Sleep -Milliseconds 300
  }
}
Set-Content -LiteralPath $recoveryError -Value 'The update could not be installed. The previous version is still in use.' -Encoding UTF8 -ErrorAction SilentlyContinue
if (Test-Path -LiteralPath $current) { Start-Process -FilePath $current }
Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
exit 1
"#;

#[cfg(windows)]
const WINDOWS_NAME_MIGRATION_SCRIPT: &str = r#"param([switch]$Elevated)
$ErrorActionPreference = 'Stop'
$procId = [int]$env:CEDAR_MIGRATION_PID
$current = $env:CEDAR_MIGRATION_CURRENT
$target = $env:CEDAR_MIGRATION_TARGET
$scriptPath = $env:CEDAR_MIGRATION_SCRIPT
$log = $env:CEDAR_MIGRATION_LOG
while (Get-Process -Id $procId -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 200 }
try {
  if (Test-Path -LiteralPath $target) { throw 'Cedar.exe already exists beside the legacy executable.' }
  Move-Item -LiteralPath $current -Destination $target
  Start-Process -FilePath $target
  Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
  exit 0
} catch {
  Add-Content -LiteralPath $log -Value "$(Get-Date -Format s) executable-name migration failed: $($_.Exception.Message)" -ErrorAction SilentlyContinue
  if (-not $Elevated) {
    try {
      Start-Process -FilePath 'powershell.exe' -Verb RunAs -WindowStyle Hidden -ArgumentList @(
        '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden',
        '-File', "`"$scriptPath`"", '-Elevated'
      )
      exit 0
    } catch {
      Add-Content -LiteralPath $log -Value "$(Get-Date -Format s) executable-name migration elevation failed: $($_.Exception.Message)" -ErrorAction SilentlyContinue
    }
  }
}
if (-not (Test-Path -LiteralPath $current) -and (Test-Path -LiteralPath $target)) {
  Move-Item -LiteralPath $target -Destination $current -Force -ErrorAction SilentlyContinue
}
$env:CEDAR_SKIP_NAME_MIGRATION = '1'
if (Test-Path -LiteralPath $current) { Start-Process -FilePath $current }
Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
exit 1
"#;

#[cfg(target_os = "macos")]
const MACOS_INSTALL_SCRIPT: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
rm -rf "$4"
mv "$3" "$4"
if mv "$2" "$3"; then
  "$3/Contents/MacOS/Cedar" &
  updated_pid=$!
  attempts=0
  while [ "$attempts" -lt 100 ]; do
    if [ -f "$CEDAR_UPDATE_HEALTH_PATH" ] && [ "$(cat "$CEDAR_UPDATE_HEALTH_PATH")" = "$CEDAR_UPDATE_HEALTH_NONCE" ]; then
      rm -rf "$4"
      rm -f "$CEDAR_UPDATE_HEALTH_PATH"
      exit 0
    fi
    if ! kill -0 "$updated_pid" 2>/dev/null; then break; fi
    attempts=$((attempts + 1))
    sleep 0.2
  done
  kill "$updated_pid" 2>/dev/null || true
fi
rm -rf "$3"
mv "$4" "$3"
printf '%s\n' 'The update failed and the previous version was restored.' > "$CEDAR_UPDATE_RECOVERY_ERROR"
printf '%s\n' 'The updated process did not report healthy; restored the previous app bundle.' >> "$CEDAR_UPDATE_LOG"
rm -f "$CEDAR_UPDATE_HEALTH_PATH"
unset CEDAR_UPDATE_HEALTH_PATH CEDAR_UPDATE_HEALTH_NONCE
"$3/Contents/MacOS/Cedar" &
exit 1"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versioned_release_tags() {
        assert_eq!(
            parse_release_version("v1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
        assert!(parse_release_version("latest").is_err());
    }

    #[test]
    fn checksum_parser_requires_the_exact_asset() {
        let expected = "a".repeat(64);
        let manifest = format!("{}  Cedar.exe\n{}  other.zip", expected, "b".repeat(64));
        assert_eq!(
            checksum_for_asset(&manifest, "Cedar.exe").unwrap(),
            expected
        );
        assert!(checksum_for_asset(&manifest, "Cedar_windows-x64.zip").is_err());
    }

    #[test]
    fn update_urls_are_scoped_to_cedar_releases() {
        assert!(
            validate_download_url(
                "https://github.com/oshtz/cedar/releases/download/v1.2.3/Cedar.exe"
            )
            .is_ok()
        );
        assert!(
            validate_download_url("http://github.com/oshtz/cedar/releases/download/v1/a").is_err()
        );
        assert!(
            validate_download_url("https://github.com/other/cedar/releases/download/v1/a").is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_updater_uses_the_stable_asset_name() {
        assert_eq!(package_asset_name().unwrap(), "Cedar.exe");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_updater_uses_the_stable_asset_name() {
        assert_eq!(package_asset_name().unwrap(), "Cedar_macos.app.zip");
    }

    #[cfg(windows)]
    #[test]
    fn legacy_windows_release_names_migrate_to_cedar_exe() {
        assert_eq!(
            windows_install_target(Path::new(r"C:\Downloads\Cedar_0.2.3_windows-x64.exe")),
            PathBuf::from(r"C:\Downloads\Cedar.exe")
        );
        assert_eq!(
            windows_install_target(Path::new(r"C:\Downloads\Cedar.exe")),
            PathBuf::from(r"C:\Downloads\Cedar.exe")
        );
        assert_eq!(
            windows_install_target(Path::new(r"C:\Downloads\cedar.exe")),
            PathBuf::from(r"C:\Downloads\cedar.exe")
        );
        assert_eq!(
            windows_install_target(Path::new(r"C:\Downloads\Cedar-custom.exe")),
            PathBuf::from(r"C:\Downloads\Cedar-custom.exe")
        );
        assert_eq!(
            windows_install_target(Path::new(r"C:\Downloads\Cedar_custom_windows-x64.exe")),
            PathBuf::from(r"C:\Downloads\Cedar_custom_windows-x64.exe")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_signature_check_accepts_an_unsigned_local_build() {
        let executable = std::env::current_exe().unwrap();
        let result = verify_platform_signature(&executable).unwrap();
        assert!(result.starts_with("SHA-256"));
    }
}
