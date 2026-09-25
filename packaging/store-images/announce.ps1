# Draws the image to share that the app is out on the Store: 2400 x 1254,
# which is 1200 x 627 at twice the density. Uses the screenshot capture.ps1
# took for the first Store image.
#
# Writes packaging\store-images\announce.png.

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$shot = Join-Path $root "target\store-images\raw\1-plan.png"
if (-not (Test-Path $shot)) { throw "no 1-plan.png yet; run capture.ps1 first" }
$work = Join-Path $root "target\store-images\announce"
New-Item -ItemType Directory -Force $work | Out-Null
$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"

# Beside the page, since a headless Edge won't load a picture from elsewhere.
Copy-Item $shot (Join-Path $work "shot.png") -Force
Copy-Item (Join-Path $root "packaging\Assets\Square150x150Logo.png") (Join-Path $work "logo.png") -Force
$page = Join-Path $work "announce.html"
Copy-Item (Join-Path $PSScriptRoot "announce.html") $page -Force
$out = Join-Path $PSScriptRoot "announce.png"
& $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 --window-size=1200,627 --allow-file-access-from-files "--screenshot=$out" ([Uri]$page).AbsoluteUri 2>$null | Out-Null
Write-Host "wrote $out"
