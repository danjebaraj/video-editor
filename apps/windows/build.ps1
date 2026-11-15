<#
.SYNOPSIS
  Builds, tests and packages Syncfusion Video Studio for Windows (ui/spec.md §23).

.DESCRIPTION
  .\build.ps1 debug      Web UI + debug binaries, laid out in target\debug\app\
  .\build.ps1 release    Web UI + release binaries (x64 and ARM64), laid out in target\<triple>\release\app\
  .\build.ps1 selftest   Debug build, then the in-app self test (VIDEO_STUDIO_SELFTEST); non-zero on failure
  .\build.ps1 package    Release build, MSIX per architecture, the .msixbundle, signing, portable CLI zips
  .\build.ps1 uitest     Debug build, then the UI Automation tests (vs-uitest)
  .\build.ps1 test       fmt, clippy -D warnings and cargo test for the workspace

  Signing (package): set VIDEO_STUDIO_SIGN_PFX and VIDEO_STUDIO_SIGN_PASSWORD for the Syncfusion
  certificate. Without them a self-signed test certificate is made (once) and its trust steps printed.
#>
param(
  [Parameter(Position = 0)][ValidateSet('debug', 'release', 'selftest', 'package', 'uitest', 'test')][string]$Mode = 'debug',
  [string[]]$Arch = @('x64', 'arm64'),
  [switch]$SkipUi
)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Resolve-Path (Join-Path $here '..\..')
$triples = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }

function Step($text) { Write-Host "`n▸ $text" -ForegroundColor Cyan }
function Run([string]$exe, [string[]]$argv) {
  # Native tools write progress to stderr; don't let 'Stop' turn that into a failure.
  $ErrorActionPreference = 'Continue'
  & $exe @argv
  if ($LASTEXITCODE -ne 0) { throw "$exe $($argv -join ' ') failed with exit $LASTEXITCODE" }
}

function Sdk-Tool([string]$name) {
  $cmd = Get-Command $name -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $found = Get-ChildItem $kits -Directory -Filter '10.*' -ErrorAction SilentlyContinue | Sort-Object Name -Descending |
    ForEach-Object { Join-Path $_.FullName "x64\$name" } | Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $found) { throw "$name wasn't found: install the Windows 11 SDK (Visual Studio Build Tools)." }
  return $found
}

function Build-Ui {
  if ($SkipUi) { return }
  Step 'Web UI (TypeScript + Vite)'
  Push-Location (Join-Path $here 'ui')
  try {
    if (-not (Test-Path node_modules)) { Run npm @('ci', '--no-audit', '--no-fund') }
    Run npm @('run', 'build')
  } finally { Pop-Location }
}

function Build-Rust([string]$profile, [string]$triple) {
  $argv = @('build', '-p', 'video-studio-win', '-p', 'video-studio-cli')
  # cutline is a test dependency (the self test's assets); the package doesn't carry it.
  if ($profile -eq 'release') { $argv += '--release' } else { $argv += @('-p', 'cutline-cli') }
  if ($triple) { $argv += @('--target', $triple) }
  # ring and aws-lc-sys (TLS) compile their ARM64 code with clang: use Visual Studio's (the
  # "C++ Clang tools" component) when none is on PATH.
  if ($triple -like 'aarch64*' -and -not (Get-Command clang -ErrorAction SilentlyContinue)) {
    $vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath 2>$null
    $llvm = if ($vs) { Join-Path $vs 'VC\Tools\Llvm\x64\bin' }
    if ($llvm -and (Test-Path (Join-Path $llvm 'clang.exe'))) { $env:PATH = "$llvm;$env:PATH" }
    else { throw "An ARM64 build needs clang: install LLVM (winget install LLVM.LLVM) or the Visual Studio component 'C++ Clang tools for Windows', or build with -Arch x64." }
  }
  Step "Rust ($profile$(if ($triple) { ", $triple" }))"
  Push-Location $root
  try { Run cargo $argv } finally { Pop-Location }
}

# The installed layout: the app, the helper (also the CLI), the test doubles in debug, and ui\.
function Layout([string]$bin, [string]$dest, [bool]$withTestBins) {
  Step "Layout $dest"
  if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
  New-Item -ItemType Directory -Force $dest | Out-Null
  Copy-Item (Join-Path $bin 'video-studio-app.exe') (Join-Path $dest 'Syncfusion Video Studio.exe')
  Copy-Item (Join-Path $bin 'video-studio.exe') $dest
  if ($withTestBins) {
    foreach ($b in 'vs-fake-claude.exe', 'vs-fake-cutline.exe', 'vs-capture-target.exe', 'vs-uitest.exe', 'cutline.exe') {
      $p = Join-Path $bin $b
      if (Test-Path $p) { Copy-Item $p $dest }
    }
  }
  Copy-Item -Recurse (Join-Path $here 'ui\dist') (Join-Path $dest 'ui')
}

function Make-Assets([string]$dir) {
  Add-Type -AssemblyName System.Drawing
  New-Item -ItemType Directory -Force $dir | Out-Null
  $src = [System.Drawing.Image]::FromFile((Join-Path $root 'apps\macos\Resources\Assets.xcassets\AppIcon.appiconset\icon_512.png'))
  $sizes = @{ 'Square44x44Logo.png' = @(44, 44); 'Square150x150Logo.png' = @(150, 150); 'StoreLogo.png' = @(50, 50); 'FileIcon.png' = @(44, 44); 'Wide310x150Logo.png' = @(310, 150) }
  foreach ($name in $sizes.Keys) {
    $w, $h = $sizes[$name]
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = 'HighQualityBicubic'
    $s = [Math]::Min($w, $h)
    $g.DrawImage($src, [int](($w - $s) / 2), [int](($h - $s) / 2), $s, $s)
    $bmp.Save((Join-Path $dir $name), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
  }
  $src.Dispose()
}

function Version {
  $toml = Get-Content (Join-Path $root 'Cargo.toml') -Raw
  if ($toml -match 'version = "(\d+)\.(\d+)\.(\d+)"') { return "$($matches[1]).$($matches[2]).$($matches[3]).0" }
  return '0.1.0.0'
}

function Signing-Cert {
  if ($env:VIDEO_STUDIO_SIGN_PFX) { return @{ Pfx = $env:VIDEO_STUDIO_SIGN_PFX; Password = $env:VIDEO_STUDIO_SIGN_PASSWORD; Subject = $env:VIDEO_STUDIO_SIGN_SUBJECT } }
  $subject = 'CN=Syncfusion Video Studio Test'
  $pfx = Join-Path $root 'target\package\test-signing.pfx'
  if (-not (Test-Path $pfx)) {
    Step 'Self-signed test certificate (for local installs only)'
    New-Item -ItemType Directory -Force (Split-Path $pfx) | Out-Null
    $cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature -FriendlyName 'Syncfusion Video Studio Test' `
      -CertStoreLocation 'Cert:\CurrentUser\My' -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    $pw = ConvertTo-SecureString -String 'video-studio-test' -Force -AsPlainText
    Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $pw | Out-Null
    Export-Certificate -Cert $cert -FilePath ($pfx -replace '\.pfx$', '.cer') | Out-Null
  }
  return @{ Pfx = $pfx; Password = 'video-studio-test'; Subject = $subject; Test = $true }
}

switch ($Mode) {
  'test' {
    Push-Location $root
    try {
      Step 'cargo fmt --check'; Run cargo @('fmt', '--all', '--check')
      Step 'cargo clippy -D warnings'; Run cargo @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')
      Step 'cargo test'; Run cargo @('test', '--workspace')
    } finally { Pop-Location }
  }
  'debug' {
    Build-Ui
    Build-Rust 'debug' $null
    Layout (Join-Path $root 'target\debug') (Join-Path $root 'target\debug\app') $true
    Write-Host "`nRun: target\debug\app\Syncfusion Video Studio.exe"
  }
  'selftest' {
    Build-Ui
    Build-Rust 'debug' $null
    $out = Join-Path $env:TEMP "vs-selftest-$(Get-Date -Format yyyyMMdd-HHmmss)"
    Step "Self test → $out"
    $env:VS_SELFTEST = $out
    $env:VIDEO_STUDIO_SELFTEST = '1'
    $p = Start-Process -FilePath (Join-Path $root 'target\debug\video-studio-app.exe') -ArgumentList '--selftest' -PassThru -Wait
    Remove-Item Env:\VS_SELFTEST, Env:\VIDEO_STUDIO_SELFTEST
    Get-Content (Join-Path $out 'selftest.log') -Encoding UTF8
    Write-Host "Snapshots: $out\shots"
    if (-not (Select-String -Path (Join-Path $out 'selftest.log') -Pattern '^PASS' -Quiet)) { exit 1 }
  }
  'uitest' {
    Build-Ui
    Build-Rust 'debug' $null
    Push-Location $root
    try { Run cargo @('build', '-p', 'video-studio-win', '--bin', 'vs-uitest') } finally { Pop-Location }
    Step 'UI Automation tests'
    & (Join-Path $root 'target\debug\vs-uitest.exe') --app (Join-Path $root 'target\debug\video-studio-app.exe')
    exit $LASTEXITCODE
  }
  { $_ -in 'release', 'package' } {
    Build-Ui
    foreach ($a in $Arch) {
      Build-Rust 'release' $triples[$a]
      Layout (Join-Path $root "target\$($triples[$a])\release") (Join-Path $root "target\$($triples[$a])\release\app") $false
    }
    if ($Mode -eq 'release') { break }

    $makeappx = Sdk-Tool 'makeappx.exe'
    $signtool = Sdk-Tool 'signtool.exe'
    $cert = Signing-Cert
    $version = Version
    $pkgDir = Join-Path $root 'target\package'
    New-Item -ItemType Directory -Force $pkgDir | Out-Null
    $msix = @()
    foreach ($a in $Arch) {
      $layout = Join-Path $root "target\$($triples[$a])\release\app"
      Step "MSIX ($a)"
      Make-Assets (Join-Path $layout 'Assets')
      $manifest = (Get-Content (Join-Path $here 'package\AppxManifest.xml') -Raw).Replace('$VERSION$', $version).Replace('$ARCH$', $a).Replace('$PUBLISHER$', $cert.Subject)
      [IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest)
      $out = Join-Path $pkgDir "SyncfusionVideoStudio_${version}_$a.msix"
      Run $makeappx @('pack', '/o', '/d', $layout, '/p', $out)
      Run $signtool @('sign', '/fd', 'SHA256', '/f', $cert.Pfx, '/p', $cert.Password, $out)
      $msix += $out
      # The portable CLI: video-studio.exe and the README's CLI section.
      $zipDir = Join-Path $pkgDir "cli-$a"
      if (Test-Path $zipDir) { Remove-Item -Recurse -Force $zipDir }
      New-Item -ItemType Directory -Force $zipDir | Out-Null
      Copy-Item (Join-Path $layout 'video-studio.exe') $zipDir
      $readme = Get-Content (Join-Path $root 'docs\video-studio\README.md') -Raw
      if ($readme -match '(?s)(## The CLI.*?)(\r?\n## )') { $readme = $matches[1] }
      [IO.File]::WriteAllText((Join-Path $zipDir 'README.md'), "# Syncfusion Video Studio CLI`n`n$readme")
      Compress-Archive -Path "$zipDir\*" -DestinationPath (Join-Path $pkgDir "video-studio-$($version -replace '\.0$','')-windows-$a.zip") -Force
    }
    Step 'Bundle'
    $bundleDir = Join-Path $pkgDir 'bundle-input'
    if (Test-Path $bundleDir) { Remove-Item -Recurse -Force $bundleDir }
    New-Item -ItemType Directory -Force $bundleDir | Out-Null
    $msix | ForEach-Object { Copy-Item $_ $bundleDir }
    $bundle = Join-Path $pkgDir "SyncfusionVideoStudio_$version.msixbundle"
    Run $makeappx @('bundle', '/o', '/d', $bundleDir, '/p', $bundle, '/bv', $version)
    Run $signtool @('sign', '/fd', 'SHA256', '/f', $cert.Pfx, '/p', $cert.Password, $bundle)
    Write-Host "`nPackages in $pkgDir"
    if ($cert.Test) {
      Write-Host @"

The packages are signed with a self-signed test certificate. To install them on this machine,
trust it once from an elevated PowerShell, then install the bundle:

  Import-Certificate -FilePath "$($cert.Pfx -replace '\.pfx$','.cer')" -CertStoreLocation Cert:\LocalMachine\TrustedPeople
  Add-AppxPackage "$bundle"

"@
    }
  }
}
