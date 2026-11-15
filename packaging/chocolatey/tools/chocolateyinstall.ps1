$ErrorActionPreference = 'Stop'

# Installs for all users, silently, with the Inno Setup installer from the
# GitHub release. Chocolatey picks the 64-bit installer on 64-bit Windows
# and the 32-bit one otherwise (or when installed with --x86). The package
# takes no parameters: Topos has no machine-wide settings to seed, and its
# per-user choices live in %LOCALAPPDATA%\Topos.
$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'exe'
  url            = '{{X86_SETUP_URL}}'
  checksum       = '{{X86_SETUP_SHA256}}'
  checksumType   = 'sha256'
  url64bit       = '{{X64_SETUP_URL}}'
  checksum64     = '{{X64_SETUP_SHA256}}'
  checksumType64 = 'sha256'
  softwareName   = 'Topos'
  silentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /ALLUSERS'
  validExitCodes = @(0)
}

Install-ChocolateyPackage @packageArgs
