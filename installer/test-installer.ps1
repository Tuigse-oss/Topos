# Installs, upgrades, and uninstalls Topos silently, the way winget,
# Chocolatey, and a user upgrading by hand do, and checks each step:
#
#   installer\test-installer.ps1 -Arch x64 `
#     -Old <setup.exe> -OldVersion 0.1.0 -New <setup.exe> -NewVersion 0.1.1 `
#     [-ProgramVersion 0.1.1]
#
# Run it on a Windows machine you can install software on, from an
# administrator prompt in 64-bit PowerShell (the all-users steps need
# administrator rights, and the 32-bit registry view is read by path). Old
# and New may be the same file to check a reinstall. Both must be built for
# -Arch. ProgramVersion, when given, is what the installed Topos.exe must
# report for --version (CI builds its two test installers from one exe, so
# their installer versions are not the program's).
#
# The upgrades run while Topos is open on a small temporary folder: Setup
# must close it (Restart Manager) and must not start it again. Anything
# already installed under the Topos AppId is uninstalled first, and
# %LOCALAPPDATA%\Topos must survive every uninstall.
param(
  [Parameter(Mandatory)] [ValidateSet('x64', 'x86')] [string] $Arch,
  [Parameter(Mandatory)] [string] $Old,
  [Parameter(Mandatory)] [string] $OldVersion,
  [Parameter(Mandatory)] [string] $New,
  [Parameter(Mandatory)] [string] $NewVersion,
  [string] $ProgramVersion,
  # Where no window can be shown (a service session), Topos exits at once
  # and the running-app checks cannot happen. Without this switch that is a
  # failure, so a Topos that crashes on start cannot pass as "skipped".
  [switch] $AllowNoWindow
)
$ErrorActionPreference = 'Stop'

$Old = (Resolve-Path -LiteralPath $Old).ProviderPath
$New = (Resolve-Path -LiteralPath $New).ProviderPath

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'run this from an administrator prompt; the all-users steps need it'
}
# A 32-bit PowerShell sees HKLM\Software through the 32-bit view, so an x64
# all-users entry would look like it was missing.
if ([Environment]::Is64BitOperatingSystem -and -not [Environment]::Is64BitProcess) {
  throw 'run this from 64-bit PowerShell'
}

# The Add/Remove Programs key Inno Setup writes for the installer's AppId.
# An all-users x86 install on 64-bit Windows lands in the 32-bit registry
# view (WOW6432Node); per-user entries are shared by both views.
$keyName = '{3F66430A-542C-46DD-949D-935DDC82C1A9}_is1'
$roots = [ordered]@{
  user      = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
  machine   = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
  machine32 = 'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
}
$machineView = 'machine'
$machineDir = $env:ProgramFiles
if ($Arch -eq 'x86' -and [Environment]::Is64BitOperatingSystem) {
  $machineView = 'machine32'
  $machineDir = ${env:ProgramFiles(x86)}
}
$expectedDirs = @{
  user          = Join-Path $env:LOCALAPPDATA 'Programs\Topos'
  $machineView  = Join-Path $machineDir 'Topos'
}
# The PE machine type the installed exe must have.
$expectedMachine = @{ x64 = 0x8664; x86 = 0x14c }[$Arch]
$silent = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-')

function Install([string] $setup, [string[]] $scope) {
  $process = Start-Process $setup -ArgumentList ($silent + $scope) -Wait -PassThru
  if ($process.ExitCode -ne 0) {
    throw "$(Split-Path $setup -Leaf) $scope exited with $($process.ExitCode)"
  }
}

function Installs {
  foreach ($view in $roots.Keys) {
    $path = Join-Path $roots[$view] $keyName
    if (Test-Path $path) {
      [pscustomobject]@{ View = $view; Entry = Get-ItemProperty $path }
    }
  }
}

function Same-Dir([string] $a, [string] $b) {
  $a.TrimEnd('\') -ieq $b.TrimEnd('\')
}

function Pe-Machine([string] $path) {
  $bytes = [IO.File]::ReadAllBytes($path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  [BitConverter]::ToUInt16($bytes, $pe + 4)
}

# Checks that exactly one Topos is installed, in the expected scope, at the
# expected version, with an Add/Remove Programs entry that names it
# properly, and returns its exe.
function Expect-Installed([ValidateSet('user', 'machine')] [string] $scope, [string] $version) {
  $view = $scope
  if ($scope -eq 'machine') { $view = $machineView }
  $found = @(Installs)
  if ($found.Count -ne 1) {
    throw "expected one install, found $($found.Count) ($($found.View -join ', '))"
  }
  if ($found[0].View -ne $view) { throw "expected a $view install, found $($found[0].View)" }
  $entry = $found[0].Entry
  if ($entry.DisplayVersion -ne $version) { throw "expected version $version, found $($entry.DisplayVersion)" }
  if ($entry.DisplayName -ne 'Topos') { throw "expected display name 'Topos', found '$($entry.DisplayName)'" }
  if ($entry.Publisher -ne 'Tuigse LLC') { throw "expected publisher 'Tuigse LLC', found '$($entry.Publisher)'" }
  if (-not (Same-Dir $entry.InstallLocation $expectedDirs[$view])) {
    throw "expected the install in $($expectedDirs[$view]), found $($entry.InstallLocation)"
  }
  $exe = Join-Path $entry.InstallLocation 'Topos.exe'
  if (-not (Test-Path $exe)) { throw "$exe is missing" }
  $machine = Pe-Machine $exe
  if ($machine -ne $expectedMachine) {
    throw ('{0} is built for machine 0x{1:x}, expected 0x{2:x} ({3})' -f $exe, $machine, $expectedMachine, $Arch)
  }
  Write-Host "ok: $view install, version $version, $exe"
  $exe
}

# The installed program must report the release version. Topos.exe is a
# GUI-subsystem program, which the call operator does not wait for, so it
# runs waited with redirected output instead.
function Expect-ProgramVersion([string] $exe) {
  if (-not $ProgramVersion) { return }
  $out = Join-Path $work 'version.txt'
  $err = Join-Path $work 'version-stderr.txt'
  $probe = Start-Process $exe -ArgumentList '--version' -Wait -PassThru `
    -RedirectStandardOutput $out -RedirectStandardError $err
  $actual = "$(Get-Content $out -Raw)".Trim()
  if ($probe.ExitCode -ne 0 -or $actual -ne "topos $ProgramVersion") {
    Get-Content $err
    throw "installed program reports '$actual', expected 'topos $ProgramVersion' (exit $($probe.ExitCode))"
  }
  Write-Host "ok: installed program reports $actual"
}

function Uninstall-All {
  foreach ($install in @(Installs)) {
    $uninstaller = $install.Entry.UninstallString.Trim('"')
    $exe = Join-Path $install.Entry.InstallLocation 'Topos.exe'
    Start-Process $uninstaller -ArgumentList $silent -Wait
    # The uninstaller hands over to a copy of itself and returns at once.
    $deadline = (Get-Date).AddMinutes(2)
    while (((Test-Path $exe) -or @(Installs).Count -gt 0) -and (Get-Date) -lt $deadline) {
      Start-Sleep -Seconds 1
    }
    if (Test-Path $exe) { throw "uninstalling left $exe behind" }
  }
  if (@(Installs).Count -gt 0) { throw 'uninstalling left an Add/Remove Programs entry' }
  Write-Host 'ok: uninstalled'
}

# Starts the installed Topos on the small scan folder and returns its
# process.
function Start-Topos([string] $exe) {
  $process = Start-Process $exe -ArgumentList "`"$scan`"" -PassThru
  Start-Sleep -Seconds 3
  if ($process.HasExited) {
    $message = "Topos did not stay open (exit $($process.ExitCode)); the running-app check cannot run"
    if ($AllowNoWindow) {
      Write-Warning "$message; skipped (-AllowNoWindow)"
      return $null
    }
    throw $message
  }
  $process
}

function Running-From([string] $exe) {
  @(Get-Process Topos -ErrorAction SilentlyContinue | Where-Object { $_.Path -and (Same-Dir $_.Path $exe) })
}

function Expect-Closed($process, [string] $exe) {
  if ($null -eq $process) { return }
  if (-not $process.HasExited) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    throw 'the upgrade did not close the running Topos'
  }
  # RestartApplications=no: give a wrongly restarted Topos time to appear.
  Start-Sleep -Seconds 3
  $again = Running-From $exe
  if ($again.Count -gt 0) {
    $again | Stop-Process -Force -ErrorAction SilentlyContinue
    throw 'the upgrade restarted Topos; it must not come back (it could come back elevated)'
  }
  Write-Host 'ok: the running Topos was closed and not restarted'
}

$work = Join-Path ([IO.Path]::GetTempPath()) "topos-installer-test-$Arch"
if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory $work | Out-Null
# A folder small enough to scan at once, so Topos is idle when Setup asks
# it to close.
$scan = Join-Path $work 'scan'
New-Item -ItemType Directory (Join-Path $scan 'sub') | Out-Null
Set-Content (Join-Path $scan 'a.txt') 'topos installer test'
Set-Content (Join-Path $scan 'sub\b.txt') ('x' * 4096)

# Data Topos keeps outside its install folder must survive every uninstall.
# The scans this test starts add a cache file there; those are removed at
# the end, and nothing that was there before is touched.
$data = Join-Path $env:LOCALAPPDATA 'Topos'
$dataExisted = Test-Path $data
New-Item -ItemType Directory -Force $data | Out-Null
$before = @(Get-ChildItem -LiteralPath $data -Force | ForEach-Object Name)
$marker = Join-Path $data 'installer-test-marker.txt'
Set-Content $marker 'settings and scan caches stay'
function Expect-DataKept {
  if (-not (Test-Path $marker)) { throw "uninstalling removed $data" }
  Write-Host 'ok: %LOCALAPPDATA%\Topos kept'
}

Uninstall-All

Write-Host "== $Arch per-user install, then an upgrade with no scope switch while Topos runs (a user double-clicking the new setup)"
Install $Old @('/CURRENTUSER')
$exe = Expect-Installed user $OldVersion
$running = Start-Topos $exe
Install $New @()
$exe = Expect-Installed user $NewVersion
Expect-Closed $running $exe
Expect-ProgramVersion $exe

Write-Host "== $Arch reinstalling the same version per user (winget scope user)"
Install $New @('/CURRENTUSER')
Expect-Installed user $NewVersion | Out-Null
Uninstall-All
Expect-DataKept

Write-Host "== $Arch all-users install and upgrade while Topos runs (Chocolatey, winget scope machine)"
Install $Old @('/ALLUSERS')
$exe = Expect-Installed machine $OldVersion
$running = Start-Topos $exe
Install $New @('/ALLUSERS')
$exe = Expect-Installed machine $NewVersion
Expect-Closed $running $exe
Expect-ProgramVersion $exe

Write-Host "== $Arch reinstalling over the all-users install with no scope switch (it must stay all-users)"
Install $New @()
Expect-Installed machine $NewVersion | Out-Null
Uninstall-All
Expect-DataKept

# The suite cleans up after itself so a rerun starts clean.
Remove-Item $marker
Get-ChildItem -LiteralPath $data -Force |
  Where-Object { $before -notcontains $_.Name -and $_.Name -like '*.toposcan*' } |
  Remove-Item -Force
if (-not $dataExisted -and @(Get-ChildItem -LiteralPath $data -Force).Count -eq 0) {
  Remove-Item -LiteralPath $data
}
Remove-Item $work -Recurse -Force
Write-Host "All $Arch installer checks passed."
