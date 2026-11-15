$ErrorActionPreference = 'Stop'

# Only the all-users install this package made, found by the installer's
# AppId (its Add/Remove Programs key), in either registry view; a per-user
# install someone made themselves is theirs to remove.
$keyName = '{3F66430A-542C-46DD-949D-935DDC82C1A9}_is1'
[array]$keys = Get-UninstallRegistryKey -SoftwareName 'Topos' |
  Where-Object { $_.PSPath -match 'HKEY_LOCAL_MACHINE' -and $_.PSChildName -eq $keyName }

foreach ($key in $keys) {
  Uninstall-ChocolateyPackage -PackageName $env:ChocolateyPackageName `
    -FileType 'exe' `
    -SilentArgs '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' `
    -File ($key.UninstallString -replace '"', '')
}
