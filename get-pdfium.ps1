<#
  Downloads pdfium.dll into this folder. The build compiles it into the exe,
  so this only needs running once, before the first `cargo build`, or again
  to change pdfium versions.

  pdfium-render 0.9.4's `pdfium_latest` feature targets the chromium/7881
  pdfium API, so that build is fetched by default.
#>
param(
    [string]$Build = '7881'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/$Build/pdfium-win-x64.tgz"

$tmp = Join-Path ([IO.Path]::GetTempPath()) "pdfium-$Build"
New-Item -ItemType Directory -Force $tmp | Out-Null
$archive = Join-Path $tmp 'pdfium-win-x64.tgz'

Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
tar -xzf $archive -C $tmp

$dll = Join-Path $tmp 'bin\pdfium.dll'
if (-not (Test-Path $dll)) { throw "pdfium.dll was not where expected in the archive ($dll)." }

Copy-Item $dll (Join-Path $root 'pdfium.dll') -Force
Write-Host "pdfium.dll saved in $root. It will be compiled into the exe."
