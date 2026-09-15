<#
  Builds the Microsoft Store package, target\msix\KineticPDF_<version>_x64.msix.

  The Store build (cargo feature `store`) has no in-app updater, since the
  Store updates the app, and loads pdfium.dll from the package instead of
  unpacking a copy embedded in the exe. It builds into target\store, so the
  regular build in target\release is left alone.

      make-msix.ps1                 a package to upload to Partner Center; the Store signs it
      make-msix.ps1 -Test           also signed with a test certificate, to install on this PC
      make-msix.ps1 -IfConfigured   nothing, unless store-identity.json is filled in (for CI)

  Needs the Windows SDK (makeappx.exe, signtool.exe) and pdfium.dll
  (get-pdfium.ps1).
#>
param(
    [switch]$Test,
    [switch]$IfConfigured
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here

$identity = Get-Content (Join-Path $here 'store-identity.json') -Raw | ConvertFrom-Json
$configured = $identity.Publisher -ne 'CN=00000000-0000-0000-0000-000000000000'
if (-not $configured) {
    if ($IfConfigured) {
        Write-Host 'packaging\store-identity.json is not filled in yet; no Store package built.'
        exit 0
    }
    if (-not $Test) {
        throw 'Fill in packaging\store-identity.json from Partner Center first, or pass -Test for a package to try locally.'
    }
    Write-Warning 'store-identity.json still has placeholder values; this package can be tested but not uploaded.'
}

# Store packages have four-part versions, and the last part must be 0.
$match = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(\d+)\.(\d+)\.(\d+)"' | Select-Object -First 1).Matches[0]
$version = '{0}.{1}.{2}.0' -f $match.Groups[1].Value, $match.Groups[2].Value, $match.Groups[3].Value

# The newest Windows SDK's tools.
$tool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\10.*\x64\makeappx.exe' -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1
if (-not $tool) { throw 'makeappx.exe not found. Install the Windows SDK.' }
$makeappx = $tool.FullName
$signtool = Join-Path $tool.DirectoryName 'signtool.exe'

$dll = Join-Path $root 'pdfium.dll'
if (-not (Test-Path $dll)) { throw 'pdfium.dll is missing. Run get-pdfium.ps1 first.' }

$targetDir = Join-Path $root 'target\store'
Push-Location $root
try {
    cargo build --release --bin kinetic-pdf --features store --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)." }
} finally {
    Pop-Location
}

$out = Join-Path $root 'target\msix'
$layout = Join-Path $out 'layout'
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $layout 'Assets') | Out-Null
Copy-Item (Join-Path $targetDir 'release\kinetic-pdf.exe') $layout
Copy-Item $dll $layout
Copy-Item (Join-Path $here 'Assets\*.png') (Join-Path $layout 'Assets')

function Escape([string]$text) { [Security.SecurityElement]::Escape($text) }
if (-not $identity.DisplayName) { throw 'store-identity.json needs DisplayName: the app name reserved in Partner Center.' }
$manifest = (Get-Content (Join-Path $here 'AppxManifest.xml') -Raw).
    Replace('{DisplayName}', (Escape $identity.DisplayName)).
    Replace('{Name}', (Escape $identity.Name)).
    Replace('{Publisher}', (Escape $identity.Publisher)).
    Replace('{PublisherDisplayName}', (Escape $identity.PublisherDisplayName)).
    Replace('{Version}', $version)
[IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest, (New-Object Text.UTF8Encoding $false))

$package = Join-Path $out "KineticPDF_${version}_x64.msix"
& $makeappx pack /d $layout /p $package /o | Out-Null
if ($LASTEXITCODE -ne 0) { throw "makeappx failed (exit $LASTEXITCODE). Run it by hand for details: `"$makeappx`" pack /d `"$layout`" /p `"$package`" /o" }
Write-Host "Built $package"

if ($Test) {
    # A self-signed certificate whose subject is the package's publisher, as
    # Windows requires, kept in the current user's store and reused.
    $cert = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $identity.Publisher -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) } |
        Select-Object -First 1
    if (-not $cert) {
        $cert = New-SelfSignedCertificate -Type Custom -Subject $identity.Publisher -KeyUsage DigitalSignature `
            -FriendlyName 'Kinetic PDF test package' -CertStoreLocation Cert:\CurrentUser\My `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    }
    & $signtool sign /q /fd SHA256 /sha1 $cert.Thumbprint $package
    if ($LASTEXITCODE -ne 0) { throw "signtool failed (exit $LASTEXITCODE)." }
    $cer = Join-Path $out 'test-certificate.cer'
    Export-Certificate -Cert $cert -FilePath $cer | Out-Null
    Write-Host ''
    Write-Host 'Signed with a test certificate. To install it on this PC, trust the certificate once, as administrator:'
    Write-Host "    Import-Certificate -FilePath `"$cer`" -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
    Write-Host 'then install the package:'
    Write-Host "    Add-AppxPackage `"$package`""
}
