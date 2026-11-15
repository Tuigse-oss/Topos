# Builds one architecture's unsigned MSIX package for the Microsoft Store:
#
#   packaging\msix\build-msix.ps1 -Version 0.1.0 -Arch x64 `
#     -SourceDir package-x64 -ArtDir installer-art -OutDir msix
#
# writes msix\Topos-v0.1.0-x64.msix. SourceDir holds the release zip's files
# (Topos.exe, the licenses, the third-party notices); ArtDir is what
# `cargo run --release --example art -- installer-art` writes. The Store
# signs what it distributes, so the package is uploaded unsigned; to
# install it locally, sign a copy (installer\test-msix.ps1 does).
#
# The identity comes from Partner Center (Product identity). Without it the
# package gets clearly test-only values: fine for CI and local tests, but
# the Store rejects them.
#
# Needs the Windows SDK (makepri.exe, makeappx.exe). Runs in Windows
# PowerShell 5.1 or PowerShell 7.
param(
  [Parameter(Mandatory)] [string] $Version,
  [Parameter(Mandatory)] [ValidateSet('x64', 'x86')] [string] $Arch,
  [Parameter(Mandatory)] [string] $SourceDir,
  [Parameter(Mandatory)] [string] $ArtDir,
  [Parameter(Mandatory)] [string] $OutDir,
  [string] $IdentityName,
  [string] $Publisher,
  [string] $PublisherDisplayName
)
$ErrorActionPreference = 'Stop'

$testIdentityName = 'TUIGSE.Topos.TestOnly'
$testPublisher = 'CN=TUIGSE Topos Test Only'
if (-not $IdentityName -or -not $Publisher) {
  $note = "MSIX identity not set; using the test-only $testIdentityName / $testPublisher, which the Store will reject"
  if ($env:GITHUB_ACTIONS -eq 'true') { Write-Host "::warning::$note" } else { Write-Warning $note }
  $IdentityName = $testIdentityName
  $Publisher = $testPublisher
}
if (-not $PublisherDisplayName) { $PublisherDisplayName = 'Tuigse LLC' }

# MSIX versions are four numbers of at most 65535; the Store keeps the last
# one at 0, so the release version must be plain X.Y.Z.
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$' -or
    @($Matches[1], $Matches[2], $Matches[3] | Where-Object { [int64]$_ -gt 65535 }).Count -gt 0) {
  throw "MSIX needs a plain X.Y.Z version with parts up to 65535, not '$Version'"
}
# Partner Center's values have these shapes; catching a pasted typo here is
# cheaper than a rejected upload.
if ($IdentityName -notmatch '^[A-Za-z0-9.-]{3,50}$') { throw "invalid package identity name '$IdentityName'" }
if ($Publisher -notmatch '^CN=') { throw "the package publisher must be a distinguished name starting with CN=, not '$Publisher'" }

# The newest Windows SDK that has both tools, x64 build.
function Find-SdkTool([string] $name) {
  $bin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $found = Get-ChildItem -LiteralPath $bin -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^\d+(\.\d+){3}$' -and (Test-Path (Join-Path $_.FullName "x64\$name")) } |
    Sort-Object { [version]$_.Name } -Descending |
    Select-Object -First 1
  if (-not $found) { throw "$name not found under $bin; install the Windows SDK" }
  Join-Path $found.FullName "x64\$name"
}
$makepri = Find-SdkTool 'makepri.exe'
$makeappx = Find-SdkTool 'makeappx.exe'
Write-Host "makepri: $makepri"
Write-Host "makeappx: $makeappx"

$SourceDir = (Resolve-Path -LiteralPath $SourceDir).ProviderPath
$ArtDir = (Resolve-Path -LiteralPath $ArtDir).ProviderPath
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).ProviderPath

# The package's files: the program, the licenses and notices the release
# zip ships with, the manifest, and the logos.
$layout = Join-Path $OutDir "layout-$Arch"
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
New-Item -ItemType Directory $layout | Out-Null
foreach ($file in 'Topos.exe', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD-PARTY-NOTICES.txt') {
  Copy-Item -LiteralPath (Join-Path $SourceDir $file) -Destination $layout
}
$assets = Join-Path $ArtDir 'msix\Assets'
Copy-Item -LiteralPath $assets -Destination (Join-Path $layout 'Assets') -Recurse
# Every logo the manifest names needs its scaled variants; a missing one
# would otherwise show up only as a blank tile after install.
foreach ($logo in 'StoreLogo', 'Square44x44Logo', 'Square150x150Logo', 'Wide310x150Logo') {
  foreach ($scale in 100, 125, 150, 200, 400) {
    $path = Join-Path $layout "Assets\$logo.scale-$scale.png"
    if (-not (Test-Path $path)) { throw "missing $path (from $assets)" }
  }
}
foreach ($size in 16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256) {
  foreach ($suffix in '', '_altform-unplated', '_altform-lightunplated') {
    $path = Join-Path $layout "Assets\Square44x44Logo.targetsize-$size$suffix.png"
    if (-not (Test-Path $path)) { throw "missing $path (from $assets)" }
  }
}

# The manifest, with values escaped for XML and no placeholder left over.
$template = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $PSScriptRoot 'AppxManifest.xml')
$values = [ordered]@{
  '{{IDENTITY_NAME}}'          = $IdentityName
  '{{PUBLISHER}}'              = $Publisher
  '{{PUBLISHER_DISPLAY_NAME}}' = $PublisherDisplayName
  '{{VERSION}}'                = $Version
  '{{ARCH}}'                   = $Arch
}
$manifest = $template
foreach ($key in $values.Keys) {
  $manifest = $manifest.Replace($key, [Security.SecurityElement]::Escape($values[$key]))
}
if ($manifest -match '\{\{[A-Z_]+\}\}') { throw "AppxManifest.xml has a placeholder this script does not fill: $($Matches[0])" }
[xml] $parsed = $manifest
$identity = $parsed.Package.Identity
Write-Host "identity: $($identity.Name), $($identity.Publisher), $($identity.Version), $($identity.ProcessorArchitecture)"
[IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest, (New-Object Text.UTF8Encoding $false))

# resources.pri indexes the qualified logo files (scale-200,
# targetsize-48_altform-unplated, ...) so the shell can pick the right one
# for each unqualified name in the manifest. The config stays outside the
# layout so it is not packaged.
$priConfig = Join-Path $OutDir "priconfig-$Arch.xml"
& $makepri createconfig /cf $priConfig /dq en-US /pv 10.0.0 /o
if ($LASTEXITCODE -ne 0) { throw "makepri createconfig exited with $LASTEXITCODE" }
# The default config splits each scale into its own resources.scale-N.pri,
# meant for separate resource packages. This is one package, so every
# candidate goes into the one resources.pri instead.
[xml] $config = Get-Content -Raw -LiteralPath $priConfig
$packaging = $config.SelectSingleNode('/resources/packaging')
if ($packaging) { [void] $config.DocumentElement.RemoveChild($packaging) }
$config.Save($priConfig)
& $makepri new /pr $layout /cf $priConfig /mn (Join-Path $layout 'AppxManifest.xml') /of (Join-Path $layout 'resources.pri') /o
if ($LASTEXITCODE -ne 0) { throw "makepri new exited with $LASTEXITCODE" }
$split = @(Get-ChildItem -LiteralPath $layout -Filter 'resources.*.pri')
if ($split.Count -gt 0) { throw "makepri split the resources: $($split.Name -join ', ')" }

$package = Join-Path $OutDir "Topos-v$Version-$Arch.msix"
& $makeappx pack /d $layout /p $package /o
if ($LASTEXITCODE -ne 0) { throw "makeappx pack exited with $LASTEXITCODE" }
Write-Host "ok: $package"
Get-FileHash -LiteralPath $package -Algorithm SHA256
