[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$InputExe,

  [Parameter(Mandatory = $true)]
  [string]$OutputExe,

  [Parameter(Mandatory = $true)]
  [string]$ProjectFile,

  [string]$StagingDir = "",

  [switch]$RequireControlledDownload
)

$ErrorActionPreference = "Stop"

function Resolve-FullPath([string]$Path) {
  return $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
}

function Find-EnigmaConsole {
  $candidates = @()

  if ($env:ENIGMA_VIRTUAL_BOX_CONSOLE) {
    $candidates += $env:ENIGMA_VIRTUAL_BOX_CONSOLE
  }

  $command = Get-Command enigmavbconsole.exe -ErrorAction SilentlyContinue
  if ($command) {
    $candidates += $command.Source
  }

  if (${env:ProgramFiles(x86)}) {
    $candidates += Join-Path ${env:ProgramFiles(x86)} "Enigma Virtual Box\enigmavbconsole.exe"
  }

  if ($env:ProgramFiles) {
    $candidates += Join-Path $env:ProgramFiles "Enigma Virtual Box\enigmavbconsole.exe"
  }

  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path -LiteralPath $candidate)) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }

  return $null
}

function Install-EnigmaVirtualBox {
  $url = $env:ENIGMA_VIRTUAL_BOX_INSTALLER_URL
  $expectedHash = $env:ENIGMA_VIRTUAL_BOX_INSTALLER_SHA256
  if (-not $expectedHash) {
    $expectedHash = $env:EVB_INSTALLER_SHA256
  }

  if (-not $url) {
    throw "ENIGMA_VIRTUAL_BOX_INSTALLER_URL is required when enigmavbconsole.exe is not already installed."
  }

  if ($RequireControlledDownload -and -not $expectedHash) {
    throw "ENIGMA_VIRTUAL_BOX_INSTALLER_SHA256 is required when controlled Enigma downloads are enforced."
  }

  $installer = Join-Path $env:RUNNER_TEMP "enigmavb-installer.exe"
  Invoke-WebRequest -Uri $url -OutFile $installer

  if ($expectedHash) {
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
    $expectedHashes = @($expectedHash -split "[,;\s]+" | Where-Object { $_ } | ForEach-Object { $_.Trim().ToLowerInvariant() })
    if ($expectedHashes -notcontains $actualHash) {
      throw "Enigma installer SHA-256 mismatch. Expected one of $($expectedHashes -join ', '), got $actualHash."
    }
  }

  $process = Start-Process -FilePath $installer -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-" -Wait -PassThru
  if ($process.ExitCode -ne 0) {
    throw "Enigma Virtual Box installer failed with exit code $($process.ExitCode)."
  }
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$inputPath = Resolve-FullPath $InputExe
$outputPath = Resolve-FullPath $OutputExe
$projectPath = Resolve-FullPath $ProjectFile

if (-not (Test-Path -LiteralPath $inputPath)) {
  throw "Input executable does not exist: $inputPath"
}

if (-not $StagingDir) {
  $StagingDir = Join-Path $env:RUNNER_TEMP "cedar-portable-staging"
}

$stagingPath = Resolve-FullPath $StagingDir
New-Item -ItemType Directory -Force -Path $stagingPath | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outputPath) | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $projectPath) | Out-Null

Get-ChildItem -LiteralPath (Split-Path -Parent $inputPath) -File -Filter "*.dll" |
  ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stagingPath $_.Name) -Force
  }

$markerPath = Join-Path $stagingPath "cedar-portable-runtime.txt"
Set-Content -LiteralPath $markerPath -Value "Cedar portable runtime bundle." -NoNewline

$console = Find-EnigmaConsole
if (-not $console) {
  Install-EnigmaVirtualBox
  $console = Find-EnigmaConsole
}

if (-not $console) {
  throw "enigmavbconsole.exe was not found after installation."
}

node (Join-Path $repoRoot "scripts\generate-evb.mjs") --project $projectPath --input $inputPath --output $outputPath --pack $stagingPath

& $console $projectPath
if ($LASTEXITCODE -ne 0) {
  throw "Enigma Virtual Box failed with exit code $LASTEXITCODE."
}

if (-not (Test-Path -LiteralPath $outputPath)) {
  throw "Portable executable was not created: $outputPath"
}

$bytes = (Get-Item -LiteralPath $outputPath).Length
if ($bytes -lt 1048576) {
  throw "Portable executable is implausibly small: $bytes bytes."
}

Write-Host "Created portable executable: $outputPath ($bytes bytes)"
