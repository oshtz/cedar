[CmdletBinding()]
param(
  [string]$OutputDirectory = "dist-release/windows",
  [switch]$SkipBuild,
  [switch]$RequireSignature,
  [switch]$SkipSmokeTest
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$releaseDirectory = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
  [IO.Path]::GetFullPath($OutputDirectory)
} else {
  [IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDirectory))
}

Push-Location $repoRoot
try {
  if (-not $SkipBuild) {
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
      throw "Cedar release build failed."
    }
  }

  $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to read Cargo package metadata."
  }
  $version = $metadata.packages[0].version
  $builtExecutable = Join-Path $repoRoot "target/release/cedar.exe"
  if (-not (Test-Path -LiteralPath $builtExecutable -PathType Leaf)) {
    throw "Release executable was not found at $builtExecutable."
  }

  New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
  $standaloneName = "Cedar.exe"
  $zipName = "Cedar_windows-x64.zip"
  $legacyUpdaterName = "Cedar_${version}_windows-x64.exe"
  $checksumName = "SHA256SUMS-windows.txt"
  $standalonePath = Join-Path $releaseDirectory $standaloneName
  $zipPath = Join-Path $releaseDirectory $zipName
  $legacyUpdaterPath = Join-Path $releaseDirectory $legacyUpdaterName
  $checksumPath = Join-Path $releaseDirectory $checksumName

  Get-ChildItem -LiteralPath $releaseDirectory -File | Where-Object {
    $_.Name -match '^Cedar_.+_windows-x64\.(exe|zip)$' -or
    $_.Name -in @($standaloneName, $zipName, $checksumName)
  } | ForEach-Object {
    Remove-Item -LiteralPath $_.FullName -Force
  }

  Copy-Item -LiteralPath $builtExecutable -Destination $standalonePath
  $signArguments = @{ Path = $standalonePath }
  if ($RequireSignature) {
    $signArguments.Require = $true
  }
  & (Join-Path $PSScriptRoot "windows-sign.ps1") @signArguments
  Copy-Item -LiteralPath $standalonePath -Destination $legacyUpdaterPath

  $stagingDirectory = Join-Path ([IO.Path]::GetTempPath()) "cedar-package-$([guid]::NewGuid().ToString('N'))"
  try {
    New-Item -ItemType Directory -Path $stagingDirectory | Out-Null
    Copy-Item -LiteralPath $standalonePath -Destination (Join-Path $stagingDirectory "Cedar.exe")
    Copy-Item -LiteralPath (Join-Path $repoRoot "README.md") -Destination $stagingDirectory
    Copy-Item -LiteralPath (Join-Path $repoRoot "LICENSE") -Destination $stagingDirectory
    Compress-Archive -Path (Join-Path $stagingDirectory "*") -DestinationPath $zipPath -CompressionLevel Optimal
  }
  finally {
    if (Test-Path -LiteralPath $stagingDirectory) {
      Remove-Item -LiteralPath $stagingDirectory -Recurse -Force
    }
  }

  $checksumLines = @($standalonePath, $zipPath, $legacyUpdaterPath) | ForEach-Object {
    $item = Get-Item -LiteralPath $_
    "{0}  {1}" -f (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $item.Name
  }
  [IO.File]::WriteAllLines($checksumPath, $checksumLines, [Text.UTF8Encoding]::new($false))

  $verifyArguments = @{
    Directory = $releaseDirectory
    Version = $version
  }
  if ($RequireSignature) {
    $verifyArguments.RequireSignature = $true
  }
  if ($SkipSmokeTest) {
    $verifyArguments.SkipSmokeTest = $true
  }
  & (Join-Path $PSScriptRoot "verify-windows-release.ps1") @verifyArguments

  Write-Host "Packaged Cedar $version in $releaseDirectory"
  Get-ChildItem -LiteralPath $releaseDirectory -File | Select-Object Name, Length
}
finally {
  Pop-Location
}
