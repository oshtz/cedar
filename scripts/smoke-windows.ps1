[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Path,

  [ValidateRange(2, 60)]
  [int]$StartupSeconds = 8
)

$ErrorActionPreference = "Stop"

$executable = (Resolve-Path -LiteralPath $Path).Path
$versionOutput = & $executable --version
if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^Cedar \d+\.\d+\.\d+') {
  throw "Packaged executable did not report a valid version: $versionOutput"
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

  Write-Host "Packaged GPUI runtime stayed healthy for $StartupSeconds seconds ($versionOutput)."
}
finally {
  if ($process -and -not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
  }
}
