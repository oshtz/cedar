[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string[]]$Path,

  [switch]$Require
)

$ErrorActionPreference = "Stop"

function Find-SignTool {
  $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($command) {
    return $command.Source
  }

  $roots = @()
  if (${env:ProgramFiles(x86)}) {
    $roots += Join-Path ${env:ProgramFiles(x86)} "Windows Kits"
  }
  if ($env:ProgramFiles) {
    $roots += Join-Path $env:ProgramFiles "Windows Kits"
  }

  foreach ($root in $roots) {
    if (Test-Path -LiteralPath $root) {
      $candidate = Get-ChildItem -LiteralPath $root -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match "\\x64\\signtool\.exe$" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1

      if ($candidate) {
        return $candidate.FullName
      }
    }
  }

  throw "signtool.exe was not found."
}

$certificate = $env:WINDOWS_CODESIGN_CERTIFICATE
$password = $env:WINDOWS_CODESIGN_PASSWORD
if (-not $certificate) {
  if ($Require) {
    throw "WINDOWS_CODESIGN_CERTIFICATE is required."
  }

  Write-Warning "WINDOWS_CODESIGN_CERTIFICATE is not set; Windows artifacts will remain unsigned."
  exit 0
}

if (-not $password) {
  throw "WINDOWS_CODESIGN_PASSWORD is required when WINDOWS_CODESIGN_CERTIFICATE is set."
}

$temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$pfxPath = Join-Path $temporaryRoot "cedar-windows-codesign-$([guid]::NewGuid().ToString('N')).pfx"
try {
  [IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($certificate))
  $signtool = Find-SignTool

  foreach ($item in $Path) {
    $resolved = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($item)
    if (-not (Test-Path -LiteralPath $resolved)) {
      throw "Cannot sign missing file: $resolved"
    }

    & $signtool sign /f $pfxPath /p $password /fd SHA256 /tr "http://timestamp.digicert.com" /td SHA256 $resolved
    if ($LASTEXITCODE -ne 0) {
      throw "signtool sign failed for $resolved."
    }

    & $signtool verify /pa /v $resolved
    if ($LASTEXITCODE -ne 0) {
      throw "signtool verify failed for $resolved."
    }

    $signature = Get-AuthenticodeSignature -LiteralPath $resolved
    if ($signature.Status -ne "Valid") {
      throw "Authenticode signature is $($signature.Status) for $resolved."
    }

    Write-Host "Signed and verified: $resolved"
  }
}
finally {
  if (Test-Path -LiteralPath $pfxPath) {
    Remove-Item -LiteralPath $pfxPath -Force
  }
}
