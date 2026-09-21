# Frames the app screenshots capture.ps1 took as the Store images, 2560 x
# 1440: a label and a headline over a tinted drafting grid, and the app
# below, running off the bottom edge. Edge draws each one from slide.html.
#
# Writes packaging\store-images\<n>-<name>.png.

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$raw = Join-Path $root "target\store-images\raw"
$work = Join-Path $root "target\store-images\compose"
New-Item -ItemType Directory -Force $work | Out-Null
$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
$template = Get-Content (Join-Path $PSScriptRoot "slide.html") -Raw

# Each image: its screenshot, label and headline, the background's tint and
# grid lines, and how the window sits -- its width, how far down it starts,
# and how much of the screenshot's top is cut off (in the screenshot's pixels
# at that width).
$slides = @(
    @{ shot = "1-plan";       label = "Kinetic PDF"; headline = "The fast PDF viewer for drawings.";  tint = "#e9f0fa"; line = "rgba(37,99,235,0.10)";  fine = "rgba(37,99,235,0.045)"; width = 2080; top = 330; crop = 0 },
    @{ shot = "2-measure";    label = "Measure";     headline = "Scale it once. Measure anything.";   tint = "#f5eee4"; line = "rgba(180,110,40,0.12)"; fine = "rgba(180,110,40,0.05)";  width = 2080; top = 330; crop = 0 },
    @{ shot = "3-quantities"; label = "Quantities";  headline = "Every measurement, totalled.";       tint = "#e7f3ec"; line = "rgba(22,128,70,0.11)";  fine = "rgba(22,128,70,0.05)";   width = 2080; top = 330; crop = 0 },
    @{ shot = "4-speed";      label = "Speed";       headline = "Big drawing sets. No waiting.";      tint = "#eceaf8"; line = "rgba(90,70,200,0.11)";  fine = "rgba(90,70,200,0.05)";   width = 2080; top = 330; crop = 0 },
    @{ shot = "5-arrange";    label = "Arrange";     headline = "Put the set in order.";              tint = "#fbeee7"; line = "rgba(200,90,50,0.11)";  fine = "rgba(200,90,50,0.05)";   width = 2080; top = 330; crop = 0 },
    @{ shot = "6-markup";     label = "Markup";      headline = "Notes that open in any viewer.";     tint = "#fbf6de"; line = "rgba(170,130,10,0.13)"; fine = "rgba(170,130,10,0.055)"; width = 2080; top = 330; crop = 0 }
)

foreach ($s in $slides) {
    $shot = Join-Path $raw "$($s.shot).png"
    if (-not (Test-Path $shot)) { Write-Host "no $($s.shot).png yet; skipped"; continue }
    $html = $template
    foreach ($key in $s.Keys) { $html = $html.Replace("{{$key}}", [string]$s[$key]) }
    # Beside the page, since a headless Edge won't load a picture from elsewhere.
    Copy-Item $shot (Join-Path $work "$($s.shot).png") -Force
    $html = $html.Replace("{{picture}}", "$($s.shot).png")
    $page = Join-Path $work "$($s.shot).html"
    Set-Content $page $html -Encoding utf8
    $out = Join-Path $PSScriptRoot "$($s.shot).png"
    & $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --window-size=2560,1440 --allow-file-access-from-files "--screenshot=$out" ([Uri]$page).AbsoluteUri 2>$null | Out-Null
    Write-Host "wrote $out"
}
