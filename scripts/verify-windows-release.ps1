[CmdletBinding()]
param(
  [string]$Directory = "dist-release/windows",
  [string]$Version,
  [switch]$RequireSignature,
  [switch]$SkipSmokeTest
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $Version) {
  Push-Location $repoRoot
  try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
      throw "Unable to read Cargo package metadata."
    }
    $Version = $metadata.packages[0].version
  }
  finally {
    Pop-Location
  }
}

$releaseDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Directory)
$standaloneName = "Cedar_${Version}_windows-x64.exe"
$zipName = "Cedar_${Version}_windows-x64.zip"
$checksumName = "SHA256SUMS-windows.txt"
$standalonePath = Join-Path $releaseDirectory $standaloneName
$zipPath = Join-Path $releaseDirectory $zipName
$checksumPath = Join-Path $releaseDirectory $checksumName

foreach ($path in @($standalonePath, $zipPath, $checksumPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Missing release artifact: $path"
  }
}

$expectedNames = @($standaloneName, $zipName, $checksumName) | Sort-Object
$actualNames = Get-ChildItem -LiteralPath $releaseDirectory -File | Select-Object -ExpandProperty Name | Sort-Object
if (Compare-Object $expectedNames $actualNames) {
  throw "Windows release directory contains an unexpected artifact set: $($actualNames -join ', ')"
}

$checksumLines = @(Get-Content -LiteralPath $checksumPath | Where-Object { $_.Trim() })
if ($checksumLines.Count -ne 2) {
  throw "Expected two entries in $checksumName, found $($checksumLines.Count)."
}

$recorded = @{}
foreach ($line in $checksumLines) {
  if ($line -notmatch '^([0-9a-fA-F]{64})\s{2}(.+)$') {
    throw "Invalid checksum line: $line"
  }
  $recorded[$Matches[2]] = $Matches[1].ToLowerInvariant()
}

foreach ($name in @($standaloneName, $zipName)) {
  $path = Join-Path $releaseDirectory $name
  $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($recorded[$name] -ne $actualHash) {
    throw "SHA-256 mismatch for $name."
  }
}

if ($RequireSignature) {
  $signature = Get-AuthenticodeSignature -LiteralPath $standalonePath
  if ($signature.Status -ne "Valid") {
    throw "Authenticode signature is $($signature.Status) for $standaloneName."
  }
}

$extractDirectory = Join-Path ([IO.Path]::GetTempPath()) "cedar-release-verify-$([guid]::NewGuid().ToString('N'))"
try {
  Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDirectory
  $zippedExecutable = Join-Path $extractDirectory "Cedar.exe"
  foreach ($name in @("Cedar.exe", "README.md", "LICENSE")) {
    if (-not (Test-Path -LiteralPath (Join-Path $extractDirectory $name) -PathType Leaf)) {
      throw "$zipName is missing $name."
    }
  }

  $standaloneHash = (Get-FileHash -LiteralPath $standalonePath -Algorithm SHA256).Hash
  $zippedHash = (Get-FileHash -LiteralPath $zippedExecutable -Algorithm SHA256).Hash
  if ($standaloneHash -ne $zippedHash) {
    throw "The executable inside $zipName differs from $standaloneName."
  }

  if (-not $SkipSmokeTest) {
    & (Join-Path $PSScriptRoot "smoke-windows.ps1") -Path $zippedExecutable
  }
}
finally {
  if (Test-Path -LiteralPath $extractDirectory) {
    Remove-Item -LiteralPath $extractDirectory -Recurse -Force
  }
}

Write-Host "Verified Cedar $Version Windows release artifacts and checksums."
