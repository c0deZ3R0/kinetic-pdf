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

# The licences of pdfium and of the libraries built into the dll, for the
# third-party notices (make-notices.ps1). Kept in git, so they change only
# when the build does.
$licenses = Join-Path $root 'licenses\pdfium'
if (Test-Path $licenses) { Remove-Item (Join-Path $licenses '*') -Force }
New-Item -ItemType Directory -Force $licenses | Out-Null
Copy-Item (Join-Path $tmp 'LICENSE'), (Join-Path $tmp 'VERSION') $licenses -Force
Copy-Item (Join-Path $tmp 'licenses\*') $licenses -Force
Write-Host "Its licences saved in $licenses. Run make-notices.ps1 if they changed."
