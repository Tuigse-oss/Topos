# Installs a Topos MSIX package or bundle, runs it through its `topos.exe`
# alias, and removes it again, checking each step:
#
#   installer\test-msix.ps1 -Package msix\Topos-v0.1.0-x64.msix -Version 0.1.0
#   installer\test-msix.ps1 -Package Topos-v0.1.0.msixbundle -Version 0.1.0
#
# The package is built unsigned, because the Store signs what it
# distributes, and Windows installs only signed packages. So this signs a
# COPY with a throwaway self-signed certificate whose subject is the
# package's Publisher, trusts that certificate on this machine for the
# test (LocalMachine\TrustedPeople), and deletes it afterwards. The package
# given is never changed.
#
# Run it from an administrator prompt in Windows PowerShell 5.1 (the Appx
# cmdlets are native there), on a machine you can install software on; CI
# runners are discarded afterwards. It refuses to run while a package with
# the same name is installed, because removing that one would delete its
# data. Needs the Windows SDK (signtool.exe).
param(
  [Parameter(Mandatory)] [string] $Package,
  [Parameter(Mandatory)] [string] $Version
)
$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'run this from an administrator prompt; trusting the test certificate needs it'
}
$Package = (Resolve-Path -LiteralPath $Package).ProviderPath
$bundle = [IO.Path]::GetExtension($Package) -ieq '.msixbundle'

function Find-SdkTool([string] $name) {
  $bin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $found = Get-ChildItem -LiteralPath $bin -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^\d+(\.\d+){3}$' -and (Test-Path (Join-Path $_.FullName "x64\$name")) } |
    Sort-Object { [version]$_.Name } -Descending |
    Select-Object -First 1
  if (-not $found) { throw "$name not found under $bin; install the Windows SDK" }
  Join-Path $found.FullName "x64\$name"
}
$signtool = Find-SdkTool 'signtool.exe'

$work = Join-Path ([IO.Path]::GetTempPath()) 'topos-msix-test'
if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory $work | Out-Null

# The identity, read from inside the package: a package is a zip with
# AppxManifest.xml at its root, a bundle one with
# AppxMetadata/AppxBundleManifest.xml. Windows' own tar reads zips by
# content, whatever the extension, and extracts just that entry.
$contents = Join-Path $work 'contents'
New-Item -ItemType Directory $contents | Out-Null
$entry = 'AppxManifest.xml'
if ($bundle) { $entry = 'AppxMetadata/AppxBundleManifest.xml' }
& (Join-Path $env:SystemRoot 'System32\tar.exe') -xf $Package -C $contents $entry
if ($LASTEXITCODE -ne 0) { throw "could not read $entry from $Package (tar exited with $LASTEXITCODE)" }
[xml] $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $contents $entry)
if ($bundle) {
  $identity = $manifest.Bundle.Identity
} else {
  $identity = $manifest.Package.Identity
}
$name = $identity.Name
$publisher = $identity.Publisher
Write-Host "package: $name, $publisher, $($identity.Version)"
if ($identity.Version -ne "$Version.0") { throw "package version $($identity.Version), expected $Version.0" }

if (@(Get-AppxPackage -Name $name).Count -gt 0) {
  throw "$name is already installed here; remove it first (that deletes its data) or run this on a disposable machine"
}

# The signed copy, the certificate, and the install are all undone in the
# finally block, whatever fails.
$copy = Join-Path $work (Split-Path $Package -Leaf)
Copy-Item -LiteralPath $Package -Destination $copy
$cert = $null
$trusted = $null
$installed = $null
$alias = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\topos.exe'
try {
  $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $publisher `
    -CertStoreLocation Cert:\CurrentUser\My -FriendlyName 'Topos MSIX test (disposable)' `
    -NotAfter (Get-Date).AddDays(1)
  # The subject must be the manifest's Publisher exactly, or Windows
  # rejects the signature.
  if ($cert.Subject -ne $publisher) { throw "certificate subject '$($cert.Subject)' differs from publisher '$publisher'" }
  $cer = Join-Path $work 'test.cer'
  Export-Certificate -Cert $cert -FilePath $cer | Out-Null
  $trusted = Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople

  & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint $copy
  if ($LASTEXITCODE -ne 0) { throw "signtool exited with $LASTEXITCODE" }

  Add-AppxPackage -Path $copy
  $installed = Get-AppxPackage -Name $name
  if (@($installed).Count -ne 1) { throw "expected one $name package after install, found $(@($installed).Count)" }
  if ($installed.Version -ne "$Version.0") { throw "installed version $($installed.Version), expected $Version.0" }
  if (-not (Test-Path (Join-Path $installed.InstallLocation 'Topos.exe'))) { throw "Topos.exe missing from $($installed.InstallLocation)" }
  Write-Host "ok: installed $($installed.PackageFullName)"

  # The alias appears in the user's WindowsApps folder shortly after
  # registration.
  $deadline = (Get-Date).AddSeconds(30)
  while (-not (Test-Path $alias) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
  if (-not (Test-Path $alias)) { throw "the topos.exe alias did not appear at $alias" }

  # Topos.exe is a GUI-subsystem program, which the call operator does not
  # wait for, so it runs waited with redirected output instead.
  $out = Join-Path $work 'version.txt'
  $err = Join-Path $work 'version-stderr.txt'
  $probe = Start-Process -FilePath $alias -ArgumentList '--version' -Wait -PassThru `
    -RedirectStandardOutput $out -RedirectStandardError $err
  $actual = "$(Get-Content $out -Raw)".Trim()
  if ($probe.ExitCode -ne 0 -or $actual -ne "topos $Version") {
    Get-Content $err
    throw "topos.exe --version printed '$actual', expected 'topos $Version' (exit $($probe.ExitCode))"
  }
  Write-Host "ok: the topos.exe alias reports $actual"

  Remove-AppxPackage -Package $installed.PackageFullName
  $installed = $null
  if (@(Get-AppxPackage -Name $name).Count -gt 0) { throw "$name is still installed after Remove-AppxPackage" }
  $deadline = (Get-Date).AddSeconds(30)
  while ((Test-Path $alias) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
  if (Test-Path $alias) { throw "the topos.exe alias is still at $alias after removal" }
  Write-Host 'ok: removed, alias gone'
} finally {
  if ($null -ne $installed) {
    Remove-AppxPackage -Package $installed.PackageFullName -ErrorAction SilentlyContinue
  }
  if ($null -ne $trusted) {
    Remove-Item -LiteralPath "Cert:\LocalMachine\TrustedPeople\$($trusted.Thumbprint)" -ErrorAction SilentlyContinue
  }
  if ($null -ne $cert) {
    Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($cert.Thumbprint)" -DeleteKey -ErrorAction SilentlyContinue
  }
  Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
if (Test-Path "Cert:\LocalMachine\TrustedPeople\$($cert.Thumbprint)") { throw 'the test certificate is still trusted' }
Write-Host 'ok: test certificate removed'
Write-Host 'All MSIX checks passed.'
