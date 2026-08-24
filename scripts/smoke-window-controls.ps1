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
    public static extern bool GetWindowRect(IntPtr hWnd, out Rect rect);

    [StructLayout(LayoutKind.Sequential)]
    public struct Point {
        public int X;
        public int Y;
    }

    [DllImport("user32.dll")]
    public static extern bool ClientToScreen(IntPtr hWnd, ref Point point);

    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out Point point);

    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int x, int y);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr insertAfter, int x, int y, int width, int height, uint flags);

    [DllImport("user32.dll")]
    public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);

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

function Send-CedarDrag {
  param(
    [IntPtr]$Window,
    [int]$X,
    [int]$Y,
    [int]$DeltaX,
    [int]$DeltaY
  )

  $point = [CedarWindowSmoke+Point]::new()
  $point.X = $X
  $point.Y = $Y
  if (-not [CedarWindowSmoke]::ClientToScreen($Window, [ref]$point)) {
    throw "Could not translate Cedar's title-bar coordinates."
  }

  [void][CedarWindowSmoke]::SetWindowPos($Window, [IntPtr](-1), 0, 0, 0, 0, 0x0013)
  try {
    [void][CedarWindowSmoke]::SetForegroundWindow($Window)
    [void][CedarWindowSmoke]::SetCursorPos($point.X, $point.Y)
    Start-Sleep -Milliseconds 100
    [CedarWindowSmoke]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    [void][CedarWindowSmoke]::SetCursorPos($point.X + $DeltaX, $point.Y + $DeltaY)
    Start-Sleep -Milliseconds 250
    [CedarWindowSmoke]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
  }
  finally {
    [void][CedarWindowSmoke]::SetWindowPos($Window, [IntPtr](-2), 0, 0, 0, 0, 0x0013)
  }
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
$originalCursor = [CedarWindowSmoke+Point]::new()
[void][CedarWindowSmoke]::GetCursorPos([ref]$originalCursor)
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

  $windowBeforeDrag = [CedarWindowSmoke+Rect]::new()
  if (-not [CedarWindowSmoke]::GetWindowRect($window, [ref]$windowBeforeDrag)) {
    throw "Could not resolve Cedar's window position."
  }
  Send-CedarDrag -Window $window -X 300 -Y 17 -DeltaX 100 -DeltaY 60
  Wait-ForCondition `
    -Condition {
      $windowAfterDrag = [CedarWindowSmoke+Rect]::new()
      [void][CedarWindowSmoke]::GetWindowRect($window, [ref]$windowAfterDrag)
      ([Math]::Abs($windowAfterDrag.Left - $windowBeforeDrag.Left) -ge 40) -or
        ([Math]::Abs($windowAfterDrag.Top - $windowBeforeDrag.Top) -ge 40)
    } `
    -Failure "Dragging the custom title bar did not reposition Cedar."

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

  Write-Host "Verified Cedar's custom title-bar dragging and minimize, maximize, and close controls."
}
finally {
  [void][CedarWindowSmoke]::SetCursorPos($originalCursor.X, $originalCursor.Y)
  if ($process -and -not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
  }
}
