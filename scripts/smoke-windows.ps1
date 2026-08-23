[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Path,

  [ValidateRange(2, 60)]
  [int]$StartupSeconds = 8
)

$ErrorActionPreference = "Stop"

$executable = (Resolve-Path -LiteralPath $Path).Path
$version = (Get-Item -LiteralPath $executable).VersionInfo.ProductVersion
if ($version -notmatch '^\d+\.\d+\.\d+') {
  throw "Packaged executable does not contain a valid product version: $version"
}

$stream = [IO.File]::OpenRead($executable)
$reader = [IO.BinaryReader]::new($stream)
try {
  $stream.Position = 0x3c
  $peHeaderOffset = $reader.ReadInt32()
  $stream.Position = $peHeaderOffset + 24 + 68
  $subsystem = $reader.ReadUInt16()
}
finally {
  $reader.Dispose()
  $stream.Dispose()
}

if ($subsystem -ne 2) {
  throw "Packaged executable uses PE subsystem $subsystem instead of the Windows GUI subsystem."
}

$process = $null
try {
  $process = Start-Process `
    -FilePath $executable `
    -ArgumentList @("--visual-qa", "audit", "--theme", "dark", "--viewport", "1280x800") `
    -PassThru `
    -WindowStyle Hidden

  if ($process.WaitForExit($StartupSeconds * 1000)) {
    throw "Packaged Cedar exited during its startup smoke test with code $($process.ExitCode)."
  }

  Write-Host "Packaged GPUI runtime stayed healthy for $StartupSeconds seconds (Cedar $version, GUI subsystem)."
}
finally {
  if ($process -and -not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
  }
}
