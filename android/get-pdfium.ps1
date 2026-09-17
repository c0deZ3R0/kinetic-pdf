<#
  Downloads libpdfium.so for Android into libs\arm64-v8a, where cargo-apk
  packs it into the APK. The same pdfium build as ..\get-pdfium.ps1.
#>
param(
    [string]$Build = '7881'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/$Build/pdfium-android-arm64.tgz"

$tmp = Join-Path ([IO.Path]::GetTempPath()) "pdfium-android-$Build"
New-Item -ItemType Directory -Force $tmp | Out-Null
$archive = Join-Path $tmp 'pdfium-android-arm64.tgz'

Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
tar -xzf $archive -C $tmp

$so = Join-Path $tmp 'lib\libpdfium.so'
if (-not (Test-Path $so)) { throw "libpdfium.so was not where expected in the archive ($so)." }

$dest = Join-Path $root 'libs\arm64-v8a'
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item $so $dest -Force
Write-Host "libpdfium.so saved in $dest."
