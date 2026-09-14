<#
  Draws the app icon and writes, next to this script:

    icon.ico        every size Windows asks for; build.rs embeds it in the exe
    icon-128.rgba   128x128 raw RGBA, the window and taskbar icon (main.rs)
    icon.png        256x256 preview

  Each size is drawn from scratch rather than scaled down, so the small ones
  stay crisp. Re-run after changing the drawing; the outputs are checked in.
#>

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

function Color([string]$hex) { [System.Drawing.ColorTranslator]::FromHtml($hex) }
function Brush([string]$hex) { [System.Drawing.SolidBrush]::new((Color $hex)) }
function Pt([single]$x, [single]$y) { [System.Drawing.PointF]::new($x, $y) }

function New-RoundedRect([single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $d = $r * 2
    $path.AddArc($x, $y, $d, $d, 180, 90)
    $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $path
}

function Draw-Icon([int]$size) {
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.Clear([System.Drawing.Color]::Transparent)
        # Draw in a 256-unit space whatever the output size.
        $g.ScaleTransform($size / 256.0, $size / 256.0)

        # A rounded tile in the app's accent blue.
        $tile = New-RoundedRect 8 8 240 240 52
        $gradient = [System.Drawing.Drawing2D.LinearGradientBrush]::new((Pt 0 8), (Pt 0 248), (Color '#5b9bf8'), (Color '#2563eb'))
        $g.FillPath($gradient, $tile)

        # A page with its top corner folded over.
        $g.FillPolygon((Brush '#ffffff'), [System.Drawing.PointF[]]@((Pt 62 34), (Pt 158 34), (Pt 196 72), (Pt 196 222), (Pt 62 222)))
        $g.FillPolygon((Brush '#c7dafc'), [System.Drawing.PointF[]]@((Pt 158 34), (Pt 158 72), (Pt 196 72)))

        # Lines of text, one highlighted. Fewer, bolder lines at small sizes,
        # where thin ones would blur into grey.
        if ($size -le 32) {
            $lines = @(@{ Y = 104; End = 170; Lit = $false }, @{ Y = 160; End = 176; Lit = $true })
            $thick = 24
        } else {
            $lines = @(
                @{ Y = 96; End = 176; Lit = $false },
                @{ Y = 130; End = 168; Lit = $true },
                @{ Y = 164; End = 176; Lit = $false },
                @{ Y = 194; End = 136; Lit = $false }
            )
            $thick = 13
        }
        foreach ($line in $lines) {
            if ($line.Lit) {
                $pad = $thick * 0.9
                $band = New-RoundedRect 74 ($line.Y - $thick / 2 - $pad) ($line.End - 64) ($thick + 2 * $pad) 6
                $g.FillPath((Brush '#fcd34d'), $band)
            }
            $ink = if ($line.Lit) { '#1e293b' } else { '#94a3b8' }
            $g.FillPath((Brush $ink), (New-RoundedRect 84 ($line.Y - $thick / 2) ($line.End - 84) $thick ($thick / 2)))
        }
    } finally {
        $g.Dispose()
    }
    $bmp
}

# Top-down BGRA, straight alpha.
function Get-Bgra([System.Drawing.Bitmap]$bmp) {
    $rect = [System.Drawing.Rectangle]::new(0, 0, $bmp.Width, $bmp.Height)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $bytes = [byte[]]::new($bmp.Width * $bmp.Height * 4)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
        , $bytes
    } finally {
        $bmp.UnlockBits($data)
    }
}

function Get-PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = [IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    , $ms.ToArray()
}

# The classic icon image format -- a bottom-up 32-bit DIB followed by an
# all-zero AND mask -- which every Windows tool reads for the smaller sizes.
function Get-DibBytes([System.Drawing.Bitmap]$bmp) {
    $s = $bmp.Width
    $bgra = Get-Bgra $bmp
    $maskRow = [int][math]::Ceiling($s / 32.0) * 4
    $ms = [IO.MemoryStream]::new()
    $w = [IO.BinaryWriter]::new($ms)
    $w.Write([int]40)                                # biSize
    $w.Write([int]$s)                                # biWidth
    $w.Write([int]($s * 2))                          # biHeight: image plus mask
    $w.Write([int16]1)                               # biPlanes
    $w.Write([int16]32)                              # biBitCount
    $w.Write([int]0)                                 # biCompression: BI_RGB
    $w.Write([int]($bgra.Length + $maskRow * $s))    # biSizeImage
    $w.Write([int]0); $w.Write([int]0); $w.Write([int]0); $w.Write([int]0)
    for ($row = $s - 1; $row -ge 0; $row--) { $w.Write($bgra, $row * $s * 4, $s * 4) }
    $w.Write([byte[]]::new($maskRow * $s))
    $w.Flush()
    , $ms.ToArray()
}

$sizes = @(16, 20, 24, 32, 40, 48, 64, 256)
$images = [System.Collections.Generic.List[byte[]]]::new()
foreach ($s in $sizes) {
    $bmp = Draw-Icon $s
    try {
        # 256 is stored as PNG, as Windows expects; the rest as DIBs.
        if ($s -ge 256) { $images.Add((Get-PngBytes $bmp)) } else { $images.Add((Get-DibBytes $bmp)) }
    } finally {
        $bmp.Dispose()
    }
}

$ms = [IO.MemoryStream]::new()
$w = [IO.BinaryWriter]::new($ms)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }   # 0 means 256
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$images[$i].Length); $w.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $w.Write($img) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $here 'icon.ico'), $ms.ToArray())

$bmp = Draw-Icon 128
try {
    $px = Get-Bgra $bmp
    for ($i = 0; $i -lt $px.Length; $i += 4) { $b = $px[$i]; $px[$i] = $px[$i + 2]; $px[$i + 2] = $b }
    [IO.File]::WriteAllBytes((Join-Path $here 'icon-128.rgba'), $px)
} finally {
    $bmp.Dispose()
}

$bmp = Draw-Icon 256
try { $bmp.Save((Join-Path $here 'icon.png'), [System.Drawing.Imaging.ImageFormat]::Png) } finally { $bmp.Dispose() }

Write-Host "Wrote icon.ico, icon-128.rgba and icon.png to $here"
