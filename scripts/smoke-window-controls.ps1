[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Path,

  [ValidateRange(2, 30)]
  [int]$StartupSeconds = 10
)

$ErrorActionPreference = "Stop"
$executable = (Resolve-Path -LiteralPath $Path).Path

Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class CedarWindowSmoke {
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool GetClientRect(IntPtr hWnd, out Rect rect);

    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool IsZoomed(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int command);

    [DllImport("user32.dll")]
    public static extern IntPtr SendMessage(IntPtr hWnd, uint message, IntPtr wParam, IntPtr lParam);
}
"@

function Send-CedarClick {
  param(
    [IntPtr]$Window,
    [int]$X,
    [int]$Y
  )

  $coordinates = [IntPtr](($Y -shl 16) -bor ($X -band 0xffff))
  [void][CedarWindowSmoke]::SendMessage($Window, 0x0200, [IntPtr]::Zero, $coordinates)
  [void][CedarWindowSmoke]::SendMessage($Window, 0x0201, [IntPtr]1, $coordinates)
  [void][CedarWindowSmoke]::SendMessage($Window, 0x0202, [IntPtr]::Zero, $coordinates)
}

function Wait-ForCondition {
  param(
    [scriptblock]$Condition,
    [string]$Failure,
    [int]$Milliseconds = 3000
  )

  $deadline = [DateTime]::UtcNow.AddMilliseconds($Milliseconds)
  do {
    if (& $Condition) {
      return
    }
    Start-Sleep -Milliseconds 50
  } while ([DateTime]::UtcNow -lt $deadline)
  throw $Failure
}

$process = $null
try {
  $process = Start-Process `
    -FilePath $executable `
    -ArgumentList @("--visual-qa", "settings", "--theme", "dark", "--viewport", "1280x800") `
    -PassThru

  $deadline = [DateTime]::UtcNow.AddSeconds($StartupSeconds)
  do {
    if ($process.HasExited) {
      throw "Cedar exited before its title bar could be tested."
    }
    $process.Refresh()
    $window = $process.MainWindowHandle
    if ($window -ne [IntPtr]::Zero) {
      break
    }
    Start-Sleep -Milliseconds 100
  } while ([DateTime]::UtcNow -lt $deadline)

  if ($window -eq [IntPtr]::Zero) {
    throw "Cedar did not expose a main window within $StartupSeconds seconds."
  }

  $rect = [CedarWindowSmoke+Rect]::new()
  if (-not [CedarWindowSmoke]::GetClientRect($window, [ref]$rect)) {
    throw "Could not resolve Cedar's client area."
  }
  $clientWidth = $rect.Right - $rect.Left
  if ($clientWidth -lt 200) {
    throw "Cedar's client area is unexpectedly narrow: $clientWidth pixels."
  }

  Send-CedarClick -Window $window -X ($clientWidth - 100) -Y 17
  Wait-ForCondition `
    -Condition { [CedarWindowSmoke]::IsIconic($window) } `
    -Failure "The custom minimize button did not minimize Cedar."

  [void][CedarWindowSmoke]::ShowWindow($window, 9)
  Wait-ForCondition `
    -Condition { -not [CedarWindowSmoke]::IsIconic($window) } `
    -Failure "Cedar did not restore after the minimize test."

  $process.Refresh()
  $window = $process.MainWindowHandle
  $rect = [CedarWindowSmoke+Rect]::new()
  [void][CedarWindowSmoke]::GetClientRect($window, [ref]$rect)
  $clientWidth = $rect.Right - $rect.Left
  Send-CedarClick -Window $window -X ($clientWidth - 60) -Y 17
  Wait-ForCondition `
    -Condition { [CedarWindowSmoke]::IsZoomed($window) } `
    -Failure "The custom maximize button did not maximize Cedar."

  $rect = [CedarWindowSmoke+Rect]::new()
  [void][CedarWindowSmoke]::GetClientRect($window, [ref]$rect)
  $clientWidth = $rect.Right - $rect.Left
  Send-CedarClick -Window $window -X ($clientWidth - 20) -Y 17
  if (-not $process.WaitForExit(5000)) {
    throw "The custom close button did not close Cedar."
  }

  Write-Host "Verified Cedar's custom minimize, maximize, and close controls through native window messages."
}
finally {
  if ($process -and -not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
  }
}
