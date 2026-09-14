<#
  Writes assets/THIRD-PARTY-NOTICES.txt: the licences of everything compiled
  into the exe, which the app shows under About. Run it again after changing
  dependencies or the pdfium build, and commit the result. The release
  workflow runs it too, so a release always carries current notices.

  The Rust crates come from cargo-about (settings in about.toml, layout in
  about.hbs). Install it once with:

      cargo install cargo-about --locked --features cli

  pdfium's licences, and those of the libraries built into pdfium.dll, come
  from licenses/pdfium, which get-pdfium.ps1 fills.
#>

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$utf8 = New-Object System.Text.UTF8Encoding $false
$rule = '=' * 78

$crates = Join-Path $root 'target\crate-notices.txt'
New-Item -ItemType Directory -Force (Split-Path $crates) | Out-Null
Push-Location $root
try {
    cargo about generate --fail --output-file $crates about.hbs
    if ($LASTEXITCODE -ne 0) { throw "cargo about failed (exit $LASTEXITCODE)." }
} finally {
    Pop-Location
}

$pdfium = Join-Path $root 'licenses\pdfium'
if (-not (Test-Path (Join-Path $pdfium 'LICENSE'))) {
    throw "pdfium's licences are missing from $pdfium. Run get-pdfium.ps1 first."
}
$build = (Get-Content (Join-Path $pdfium 'VERSION') -ErrorAction SilentlyContinue) -join ' '

$out = New-Object System.Text.StringBuilder
[void]$out.AppendLine('THIRD-PARTY NOTICES')
[void]$out.AppendLine($rule)
[void]$out.AppendLine()
[void]$out.AppendLine('PDF Annotate is licensed under MIT OR Apache-2.0. It includes the')
[void]$out.AppendLine('open-source software below, each under the licence shown.')
[void]$out.AppendLine()
[void]$out.AppendLine('pdfium-render is included with changes; they are described in')
[void]$out.AppendLine('vendor/pdfium-render/PATCHES.md in the source repository.')
[void]$out.AppendLine()
[void]$out.AppendLine()
[void]$out.AppendLine('PDFIUM')
[void]$out.AppendLine('======')
[void]$out.AppendLine()
[void]$out.AppendLine('pdfium.dll, from https://github.com/bblanchon/pdfium-binaries, is built')
[void]$out.AppendLine('into the app. It contains pdfium and the libraries listed after it.')
if ($build) { [void]$out.AppendLine("Build: $build") }
[void]$out.AppendLine()

$files = @(Get-Item (Join-Path $pdfium 'LICENSE')) + @(Get-ChildItem $pdfium -File | Where-Object { $_.Name -notin 'LICENSE', 'VERSION' } | Sort-Object Name)
foreach ($file in $files) {
    $name = if ($file.Name -eq 'LICENSE') { 'pdfium' } else { $file.BaseName }
    [void]$out.AppendLine('-' * 78)
    [void]$out.AppendLine($name)
    [void]$out.AppendLine()
    [void]$out.AppendLine([IO.File]::ReadAllText($file.FullName).TrimEnd())
    [void]$out.AppendLine()
}

[void]$out.AppendLine()
[void]$out.Append([IO.File]::ReadAllText($crates))

# One line ending throughout, whatever each licence file came with.
$text = $out.ToString() -replace "`r`n", "`n"
$target = Join-Path $root 'assets\THIRD-PARTY-NOTICES.txt'
[IO.File]::WriteAllText($target, $text, $utf8)
Write-Host ("Wrote {0} ({1:N0} KB)" -f $target, ((Get-Item $target).Length / 1KB))
