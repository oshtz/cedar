param(
    [string[]]$Scenario,
    [ValidateSet("dark", "light", "both")]
    [string]$Theme = "both",
    [ValidateSet("minimum", "standard", "wide", "all")]
    [string]$Viewport = "all",
    [string]$OutputDirectory = "artifacts/visual-qa",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$executable = Join-Path $repoRoot "target/release/cedar.exe"
if (-not $SkipBuild) {
    & cargo build --release --locked --manifest-path (Join-Path $repoRoot "Cargo.toml")
    if ($LASTEXITCODE -ne 0) {
        throw "Cedar release build failed."
    }
}
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Cedar release executable was not found at $executable"
}

$scenarioOutput = Join-Path ([IO.Path]::GetTempPath()) "cedar-visual-qa-$([guid]::NewGuid().ToString('N')).txt"
try {
    $scenarioProcess = Start-Process `
        -FilePath $executable `
        -ArgumentList "--list-visual-qa" `
        -RedirectStandardOutput $scenarioOutput `
        -PassThru `
        -Wait
    $availableScenarios = if (Test-Path -LiteralPath $scenarioOutput) {
        @(Get-Content -LiteralPath $scenarioOutput | Where-Object { $_.Trim() })
    } else {
        @()
    }
}
finally {
    Remove-Item -LiteralPath $scenarioOutput -Force -ErrorAction SilentlyContinue
}
if ($scenarioProcess.ExitCode -ne 0 -or $availableScenarios.Count -eq 0) {
    throw "Cedar did not return any visual QA scenarios."
}
$selectedScenarios = if ($null -ne $Scenario -and $Scenario.Count -gt 0) {
    @($Scenario | ForEach-Object { $_ -split "," } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
} else {
    $availableScenarios
}
foreach ($name in $selectedScenarios) {
    if ($name -notin $availableScenarios) {
        throw "Unknown visual QA scenario '$name'. Available scenarios: $($availableScenarios -join ', ')"
    }
}

$themes = if ($Theme -eq "both") { @("dark", "light") } else { @($Theme) }
$viewportPresets = [ordered]@{
    minimum = "1120x720"
    standard = "1440x960"
    wide = "1728x960"
}
$selectedViewports = if ($Viewport -eq "all") {
    @($viewportPresets.Keys)
} else {
    @($Viewport)
}

$outputRoot = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    [System.IO.Path]::GetFullPath($OutputDirectory)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDirectory))
}
[System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null

if (-not ("CedarVisualQaNative" -as [type])) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CedarVisualQaNative
{
    private delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);

    [StructLayout(LayoutKind.Sequential)]
    public struct Rect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr window, out Rect rect);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr window, int command);

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr window, IntPtr deviceContext, uint flags);

    [DllImport("user32.dll")]
    public static extern bool SetProcessDpiAwarenessContext(IntPtr value);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder text, int capacity);

    public static IntPtr FindWindowForProcess(int processId, string titlePrefix)
    {
        IntPtr match = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId) {
                return true;
            }
            var title = new StringBuilder(512);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString().StartsWith(titlePrefix, StringComparison.Ordinal)) {
                match = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return match;
    }
}
"@
}
[CedarVisualQaNative]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
Add-Type -AssemblyName System.Drawing

function Wait-CedarWindow {
    param([System.Diagnostics.Process]$Process)

    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    while ([DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 100
        $Process.Refresh()
        if ($Process.HasExited) {
            throw "Cedar exited before its visual QA window was ready (exit code $($Process.ExitCode))."
        }
        $window = [CedarVisualQaNative]::FindWindowForProcess($Process.Id, "Cedar Visual QA")
        if ($window -ne [IntPtr]::Zero) {
            return $window
        }
    }
    throw "Timed out waiting for Cedar's visual QA window."
}

function Save-WindowCapture {
    param(
        [IntPtr]$Window,
        [System.Diagnostics.Process]$Process,
        [string]$Path
    )

    [CedarVisualQaNative]::ShowWindow($Window, 9) | Out-Null
    [CedarVisualQaNative]::SetForegroundWindow($Window) | Out-Null
    Start-Sleep -Milliseconds 650
    $Process.Refresh()
    if ($Process.HasExited) {
        throw "Cedar exited before the visual QA frame was captured (exit code $($Process.ExitCode))."
    }

    $rect = New-Object CedarVisualQaNative+Rect
    if (-not [CedarVisualQaNative]::GetWindowRect($Window, [ref]$rect)) {
        throw "Could not read Cedar's window bounds."
    }
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    if ($width -le 0 -or $height -le 0) {
        throw "Cedar returned invalid capture bounds ${width}x${height}."
    }

    $bitmap = New-Object System.Drawing.Bitmap($width, $height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $deviceContext = $graphics.GetHdc()
        try {
            $captured = [CedarVisualQaNative]::PrintWindow($Window, $deviceContext, 2)
        }
        finally {
            $graphics.ReleaseHdc($deviceContext)
        }
        if (-not $captured) {
            $graphics.CopyFromScreen(
                $rect.Left,
                $rect.Top,
                0,
                0,
                (New-Object System.Drawing.Size($width, $height)),
                [System.Drawing.CopyPixelOperation]::SourceCopy
            )
        }
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
        return [pscustomobject]@{
            width = $width
            height = $height
        }
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

$manifest = [System.Collections.Generic.List[object]]::new()
foreach ($viewportName in $selectedViewports) {
    $viewportSize = $viewportPresets[$viewportName]
    foreach ($themeName in $themes) {
        $captureDirectory = Join-Path $outputRoot "$viewportName/$themeName"
        [System.IO.Directory]::CreateDirectory($captureDirectory) | Out-Null
        foreach ($scenarioName in $selectedScenarios) {
            $capturePath = Join-Path $captureDirectory "$scenarioName.png"
            Write-Host "Capturing $scenarioName | $themeName | $viewportSize"
            $processArguments = @(
                "--visual-qa", $scenarioName,
                "--theme", $themeName,
                "--viewport", $viewportSize
            )
            $process = Start-Process -FilePath $executable -ArgumentList $processArguments -PassThru
            try {
                $window = Wait-CedarWindow -Process $process
                $capture = Save-WindowCapture -Window $window -Process $process -Path $capturePath
                $file = Get-Item -LiteralPath $capturePath
                if ($file.Length -lt 10000) {
                    throw "Capture '$capturePath' is suspiciously small and was rejected."
                }
                $relativeFile = $file.FullName.Substring($outputRoot.TrimEnd("\").Length + 1)
                $manifest.Add([pscustomobject]@{
                    scenario = $scenarioName
                    theme = $themeName
                    viewport = $viewportName
                    viewportSize = $viewportSize
                    captureWidth = $capture.width
                    captureHeight = $capture.height
                    file = $relativeFile
                    bytes = $file.Length
                    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash
                })
            }
            finally {
                if (-not $process.HasExited) {
                    $process.CloseMainWindow() | Out-Null
                    if (-not $process.WaitForExit(2000)) {
                        Stop-Process -Id $process.Id
                        $process.WaitForExit()
                    }
                }
                $process.Dispose()
            }
        }
    }
}

$manifestPath = Join-Path $outputRoot "manifest.json"
ConvertTo-Json -InputObject $manifest.ToArray() -Depth 4 |
    Set-Content -LiteralPath $manifestPath -Encoding utf8
Write-Host "Captured $($manifest.Count) visual QA frame(s) to $outputRoot"
Write-Host "Manifest: $manifestPath"
