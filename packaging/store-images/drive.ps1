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
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
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
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
    // The process's visible window whose title has `part` in it. Not its
    // MainWindowHandle: winit's untitled event window can be taken for that.
    public static IntPtr Titled(uint pid, string part) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            var t = new System.Text.StringBuilder(256); GetWindowText(h, t, 256);
            if (p == pid && IsWindowVisible(h) && t.ToString().Contains(part)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
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
    $script:H = [IntPtr]::Zero
    while ($script:H -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 200; $script:H = [Win]::Titled([uint32]$script:Kp.Id, "Kinetic PDF") }
    $h = $script:H
    [void][Win]::ShowWindow($h, 9)
    # Size the window so its client area is exactly Width x Height.
    [Win+RECT]$w = New-Object Win+RECT; [Win+RECT]$c = New-Object Win+RECT
    [void][Win]::GetWindowRect($h, [ref]$w); [void][Win]::GetClientRect($h, [ref]$c)
    $extraW = ($w.Right - $w.Left) - $c.Right; $extraH = ($w.Bottom - $w.Top) - $c.Bottom
    Focus-Kp
    if (Get-Process glazewm -ErrorAction SilentlyContinue) {
        # A tiling window manager would size it to fit its layout, and again
        # whenever the layout changes; floating, it is left the size asked.
        & glazewm command set-floating --shown-on-top=true --x-pos 40 --y-pos 60 --width ($Width + $extraW) --height ($Height + $extraH) | Out-Null
    } else {
        [void][Win]::SetWindowPos($h, [IntPtr]::Zero, 40, 60, $Width + $extraW, $Height + $extraH, 0x0040)
    }
    Start-Sleep -Milliseconds 800
    # The frame is only known once the window is up; correct for it.
    for ($i = 0; $i -lt 4; $i++) {
        [void][Win]::GetClientRect($h, [ref]$c); [void][Win]::GetWindowRect($h, [ref]$w)
        if ($c.Right -eq $Width -and $c.Bottom -eq $Height) { break }
        [void][Win]::SetWindowPos($h, [IntPtr]::Zero, $w.Left, $w.Top, ($w.Right - $w.Left) + $Width - $c.Right, ($w.Bottom - $w.Top) + $Height - $c.Bottom, 0x0040)
        Start-Sleep -Milliseconds 500
    }
    Write-Host "client $($c.Right) x $($c.Bottom)"
}

# Picks up a window already open, for driving it from a new shell.
function Find-Kp {
    $script:Kp = Get-Process kinetic-pdf -ErrorAction SilentlyContinue | Where-Object MainWindowHandle -ne 0 | Select-Object -First 1
    $script:H = [Win]::Titled([uint32]$script:Kp.Id, "Kinetic PDF")
}

# Every key and click goes to whatever window is in front, so nothing is sent
# unless it is this one.
function Focus-Kp {
    for ($i = 0; $i -lt 10 -and [Win]::GetForegroundWindow() -ne $script:H; $i++) {
        [Win]::Bring($script:H); Start-Sleep -Milliseconds 200
    }
    if ([Win]::GetForegroundWindow() -ne $script:H) { throw "Kinetic PDF is not the window in front; stopping before sending anything" }
}

function Stop-Kp { if ($script:Kp -and -not $script:Kp.HasExited) { $script:Kp.Kill() } }

# Client-area point to the screen.
function Get-Screen([int]$X, [int]$Y) {
    $p = New-Object Win+POINT; $p.X = $X; $p.Y = $Y
    [void][Win]::ClientToScreen($script:H, [ref]$p)
    $p
}

function Move-To([int]$X, [int]$Y) { $p = Get-Screen $X $Y; [void][Win]::SetCursorPos($p.X, $p.Y); Start-Sleep -Milliseconds 60 }

# Smoothstep easing at about 60 Hz for recorded demonstrations.
function Move-Smooth([int]$X, [int]$Y, [int]$DurationMs = 650) {
    Focus-Kp
    $start = New-Object Win+POINT
    [void][Win]::GetCursorPos([ref]$start)
    $end = Get-Screen $X $Y
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ([Win]::GetForegroundWindow() -ne $script:H) { throw "Focus changed; stopping the demonstration" }
        $t = [Math]::Min(1.0, $watch.Elapsed.TotalMilliseconds / [Math]::Max(1, $DurationMs))
        $ease = $t * $t * (3 - 2 * $t)
        [void][Win]::SetCursorPos([int]($start.X + ($end.X - $start.X) * $ease), [int]($start.Y + ($end.Y - $start.Y) * $ease))
        Start-Sleep -Milliseconds 16
    } while ($t -lt 1)
}

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

# A left drag, or with -Middle the middle-button drag that moves the page.
function Drag([int]$X0, [int]$Y0, [int]$X1, [int]$Y1, [switch]$Middle) {
    Focus-Kp; Move-To $X0 $Y0
    $down, $up = if ($Middle) { 0x0020, 0x0040 } else { 0x0002, 0x0004 }
    [Win]::mouse_event($down, 0, 0, 0, [UIntPtr]::Zero)
    for ($i = 1; $i -le 20; $i++) { Move-To ($X0 + ($X1 - $X0) * $i / 20) ($Y0 + ($Y1 - $Y0) * $i / 20) }
    [Win]::mouse_event($up, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 250
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
    # Copied off the screen, with the window in front: asked to draw itself
    # into a bitmap instead, the OpenGL window often gives back black.
    Focus-Kp; Start-Sleep -Milliseconds 400
    [Win+RECT]$c = New-Object Win+RECT
    [void][Win]::GetClientRect($script:H, [ref]$c)
    $bmp = New-Object System.Drawing.Bitmap $c.Right, $c.Bottom
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $p = Get-Screen 0 0
    Focus-Kp
    $g.CopyFromScreen($p.X, $p.Y, 0, 0, $bmp.Size)
    # Black means it wasn't drawn; refuse that rather than save it.
    $lit = 0
    for ($x = 0; $x -lt $c.Right; $x += [int]($c.Right / 16)) {
        for ($y = 0; $y -lt $c.Bottom; $y += [int]($c.Bottom / 16)) { if ($bmp.GetPixel($x, $y).GetBrightness() -gt 0.1) { $lit++ } }
    }
    if ($lit -lt 10) { $g.Dispose(); $bmp.Dispose(); throw "the picture of the window came back black; is something else in front of it?" }
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Host "saved $Path ($($c.Right) x $($c.Bottom))"
}
