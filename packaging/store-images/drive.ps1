# Drives a running Kinetic PDF window for the Store screenshots: sizes it,
# sends it keys and mouse, and saves its client area as a PNG. Dot-source it:
#   . .\packaging\store-images\drive.ps1
# The window takes the focus while this runs; leave the mouse and keyboard be.

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    // Windows lets only the window in front hand the focus on, so borrow its
    // input queue for the moment it takes. No key is pressed to do it.
    public static void Bring(IntPtr h) {
        uint fg = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero), me = GetCurrentThreadId();
        AttachThreadInput(me, fg, true);
        BringWindowToTop(h); SetForegroundWindow(h);
        AttachThreadInput(me, fg, false);
    }
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
}
'@
# Real pixels throughout, whatever the display scaling.
[void][Win]::SetProcessDpiAwarenessContext([IntPtr](-4))

$script:Kp = $null

# Opens $Pdf in a copy of the app with a profile of its own under $Profile:
# its tools, page cache and name there, never the user's own. $Tools, if
# given, is the tools file it starts with.
function Start-Kp([string]$Exe, [string]$Pdf, [string]$Profile, [string]$Tools, [string]$Author = "J. Morgan", [int]$Width = 2100, [int]$Height = 1320) {
    $local = Join-Path $Profile "local"; $roaming = Join-Path $Profile "roaming"
    New-Item -ItemType Directory -Force (Join-Path $local "kinetic-pdf"), (Join-Path $roaming "kinetic-pdf") | Out-Null
    if ($Tools) { Copy-Item $Tools (Join-Path $local "kinetic-pdf\tools.json") -Force }
    Set-Content (Join-Path $roaming "kinetic-pdf\author.txt") $Author -NoNewline
    $saved = $env:LOCALAPPDATA, $env:APPDATA
    $env:LOCALAPPDATA = $local; $env:APPDATA = $roaming
    try { $script:Kp = Start-Process $Exe -ArgumentList "`"$Pdf`"" -PassThru }
    finally { $env:LOCALAPPDATA, $env:APPDATA = $saved }
    while ($script:Kp.MainWindowHandle -eq 0) { Start-Sleep -Milliseconds 200; $script:Kp.Refresh() }
    $h = $script:Kp.MainWindowHandle
    [void][Win]::ShowWindow($h, 9)
    # Size the window so its client area is exactly Width x Height.
    [Win+RECT]$w = New-Object Win+RECT; [Win+RECT]$c = New-Object Win+RECT
    [void][Win]::GetWindowRect($h, [ref]$w); [void][Win]::GetClientRect($h, [ref]$c)
    $extraW = ($w.Right - $w.Left) - $c.Right; $extraH = ($w.Bottom - $w.Top) - $c.Bottom
    Focus-Kp
    if (Get-Command glazewm -ErrorAction SilentlyContinue) {
        # A tiling window manager would size it to fit its layout, and again
        # whenever the layout changes; floating, it is left the size asked.
        & glazewm command set-floating --shown-on-top=true --x-pos 40 --y-pos 60 --width ($Width + $extraW) --height ($Height + $extraH) | Out-Null
    } else {
        [void][Win]::SetWindowPos($h, [IntPtr]::Zero, 40, 60, $Width + $extraW, $Height + $extraH, 0x0040)
    }
    Start-Sleep -Milliseconds 800
    [void][Win]::GetClientRect($h, [ref]$c)
    Write-Host "client $($c.Right) x $($c.Bottom)"
}

# Picks up a window already open, for driving it from a new shell.
function Find-Kp { $script:Kp = Get-Process kinetic-pdf -ErrorAction SilentlyContinue | Where-Object MainWindowHandle -ne 0 | Select-Object -First 1 }

# Every key and click goes to whatever window is in front, so nothing is sent
# unless it is this one.
function Focus-Kp {
    for ($i = 0; $i -lt 10 -and [Win]::GetForegroundWindow() -ne $script:Kp.MainWindowHandle; $i++) {
        [Win]::Bring($script:Kp.MainWindowHandle); Start-Sleep -Milliseconds 200
    }
    if ([Win]::GetForegroundWindow() -ne $script:Kp.MainWindowHandle) { throw "Kinetic PDF is not the window in front; stopping before sending anything" }
}

function Stop-Kp { if ($script:Kp -and -not $script:Kp.HasExited) { $script:Kp.Kill() } }

# Client-area point to the screen.
function Get-Screen([int]$X, [int]$Y) {
    $p = New-Object Win+POINT; $p.X = $X; $p.Y = $Y
    [void][Win]::ClientToScreen($script:Kp.MainWindowHandle, [ref]$p)
    $p
}

function Move-To([int]$X, [int]$Y) { $p = Get-Screen $X $Y; [void][Win]::SetCursorPos($p.X, $p.Y); Start-Sleep -Milliseconds 60 }

function Click([int]$X, [int]$Y, [switch]$Right, [switch]$Double) {
    Focus-Kp; Move-To $X $Y
    $down, $up = if ($Right) { 0x0008, 0x0010 } else { 0x0002, 0x0004 }
    $times = if ($Double) { 2 } else { 1 }
    for ($i = 0; $i -lt $times; $i++) {
        [Win]::mouse_event($down, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40
        [Win]::mouse_event($up, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60
    }
    Start-Sleep -Milliseconds 250
}

function Drag([int]$X0, [int]$Y0, [int]$X1, [int]$Y1) {
    Focus-Kp; Move-To $X0 $Y0
    [Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    for ($i = 1; $i -le 20; $i++) { Move-To ($X0 + ($X1 - $X0) * $i / 20) ($Y0 + ($Y1 - $Y0) * $i / 20) }
    [Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 250
}

# Ctrl + wheel at a point: each notch zooms one step there.
function Zoom-At([int]$X, [int]$Y, [int]$Notches) {
    Focus-Kp; Move-To $X $Y
    [Win]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
    for ($i = 0; $i -lt [Math]::Abs($Notches); $i++) {
        [Win]::mouse_event(0x0800, 0, 0, [Math]::Sign($Notches) * 120, [UIntPtr]::Zero); Start-Sleep -Milliseconds 120
    }
    [Win]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 300
}

# SendKeys syntax: ^ Ctrl, + Shift, {PGDN}, {ENTER}, {ESC}.
function Keys([string]$Keys) { Focus-Kp; [System.Windows.Forms.SendKeys]::SendWait($Keys); Start-Sleep -Milliseconds 300 }

function Shot([string]$Path) {
    # The window draws itself into the bitmap, so what is on screen around
    # or over it never gets in.
    [Win+RECT]$c = New-Object Win+RECT
    [void][Win]::GetClientRect($script:Kp.MainWindowHandle, [ref]$c)
    $bmp = New-Object System.Drawing.Bitmap $c.Right, $c.Bottom
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $dc = $g.GetHdc()
    # PW_CLIENTONLY | PW_RENDERFULLCONTENT
    [void][Win]::PrintWindow($script:Kp.MainWindowHandle, $dc, 3)
    $g.ReleaseHdc($dc)
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Host "saved $Path ($($c.Right) x $($c.Bottom))"
}
