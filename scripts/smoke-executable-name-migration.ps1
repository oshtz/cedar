[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Path,

  [ValidateRange(2, 30)]
  [int]$TimeoutSeconds = 15
)

$ErrorActionPreference = "Stop"
$sourceExecutable = (Resolve-Path -LiteralPath $Path).Path
$smokeRoot = Join-Path ([IO.Path]::GetTempPath()) "cedar-name-migration-$([guid]::NewGuid().ToString('N'))"
$legacyPath = Join-Path $smokeRoot "Cedar_0.2.3_windows-x64.exe"
$canonicalPath = Join-Path $smokeRoot "Cedar.exe"
$healthNonce = [guid]::NewGuid().ToString("N")
$updateRoot = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)) "Cedar\updates"
$healthPath = Join-Path $updateRoot "update-health-smoke-$healthNonce.txt"

function Get-SmokeProcess {
  Get-Process -ErrorAction SilentlyContinue | Where-Object {
    try {
      $_.Path -eq $canonicalPath -or $_.Path -eq $legacyPath
    }
    catch {
      $false
    }
  }
}

try {
  New-Item -ItemType Directory -Path $smokeRoot | Out-Null
  New-Item -ItemType Directory -Path $updateRoot -Force | Out-Null
  Copy-Item -LiteralPath $sourceExecutable -Destination $legacyPath
  $previousHealthPath = [Environment]::GetEnvironmentVariable("CEDAR_UPDATE_HEALTH_PATH", "Process")
  $previousHealthNonce = [Environment]::GetEnvironmentVariable("CEDAR_UPDATE_HEALTH_NONCE", "Process")
  try {
    [Environment]::SetEnvironmentVariable("CEDAR_UPDATE_HEALTH_PATH", $healthPath, "Process")
    [Environment]::SetEnvironmentVariable("CEDAR_UPDATE_HEALTH_NONCE", $healthNonce, "Process")
    Start-Process -FilePath $legacyPath -WindowStyle Hidden | Out-Null
  }
  finally {
    [Environment]::SetEnvironmentVariable("CEDAR_UPDATE_HEALTH_PATH", $previousHealthPath, "Process")
    [Environment]::SetEnvironmentVariable("CEDAR_UPDATE_HEALTH_NONCE", $previousHealthNonce, "Process")
  }

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $canonicalProcess = $null
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $canonicalProcess = Get-SmokeProcess | Where-Object { $_.Path -eq $canonicalPath } | Select-Object -First 1
    if ($canonicalProcess -and
        (Test-Path -LiteralPath $canonicalPath -PathType Leaf) -and
        -not (Test-Path -LiteralPath $legacyPath)) {
      break
    }
  }

  if (-not $canonicalProcess) {
    throw "Canonical Cedar.exe did not relaunch during the executable-name migration smoke test."
  }
  if (Test-Path -LiteralPath $legacyPath) {
    throw "The legacy versioned executable remained after migration."
  }
  if (-not (Test-Path -LiteralPath $healthPath) -or
      (Get-Content -LiteralPath $healthPath -Raw) -ne $healthNonce) {
    throw "The legacy executable migrated before reporting a healthy update launch."
  }

  Write-Host "Verified healthy legacy executable migration and relaunch as Cedar.exe."
}
finally {
  Get-SmokeProcess | Stop-Process -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $healthPath -Force -ErrorAction SilentlyContinue
  $resolvedSmokeRoot = [IO.Path]::GetFullPath($smokeRoot)
  $resolvedTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if (-not $resolvedSmokeRoot.StartsWith($resolvedTempRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean unexpected migration smoke path $resolvedSmokeRoot."
  }
  if (Test-Path -LiteralPath $resolvedSmokeRoot) {
    Remove-Item -LiteralPath $resolvedSmokeRoot -Recurse -Force
  }
}
