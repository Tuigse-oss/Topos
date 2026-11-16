; Inno Setup script for the Topos installer, built by the release workflow
; once per architecture:
;   iscc /DAppVersion=0.1.0 /DTargetArch=x64 installer\topos.iss
;   iscc /DAppVersion=0.1.0 /DTargetArch=x86 installer\topos.iss
; SourceDir (default ..\package-<arch>) holds the same files as that
; architecture's release zip. ArtDir (default ..\installer-art) holds the
; icon and wizard images that `cargo run --release --example art --
; installer-art` writes; the workflows generate it before calling iscc.

#ifndef AppVersion
  #error Pass the version with /DAppVersion=x.y.z
#endif
#ifndef TargetArch
  #error Pass the target architecture with /DTargetArch=x64 or /DTargetArch=x86
#endif
#if TargetArch != "x64" && TargetArch != "x86"
  #error TargetArch must be x64 or x86
#endif
#ifndef SourceDir
  #define SourceDir "..\package-" + TargetArch
#endif
#ifndef ArtDir
  #define ArtDir "..\installer-art"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

#if TargetArch == "x64"
  #define ArchSuffix "x86_64"
  #define ArchAllowed "x64compatible"
#else
  #define ArchSuffix "x86"
  #define ArchAllowed "x86compatible"
#endif

[Setup]
; The AppId identifies the install across upgrades and across both
; architectures; never change it. A new one would install next to the old
; version instead of replacing it, and winget finds the install through the
; Add/Remove Programs key it produces ({AppId}_is1).
AppId={{3F66430A-542C-46DD-949D-935DDC82C1A9}
AppName=Topos
AppVersion={#AppVersion}
AppVerName=Topos {#AppVersion}
AppPublisher=Tuigse LLC
AppPublisherURL=https://github.com/Tuigse-oss/Topos
AppSupportURL=https://github.com/Tuigse-oss/Topos/issues
AppUpdatesURL=https://github.com/Tuigse-oss/Topos/releases
AppCopyright=Copyright (c) 2026 Tuigse LLC
DefaultDirName={autopf}\Topos
DefaultGroupName=Topos
DisableProgramGroupPage=yes
; The exe runs asInvoker, so a per-user install needs no administrator
; rights; the dialog, or /ALLUSERS for a silent install (winget machine
; scope, Chocolatey), installs for all users instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline dialog
; An upgrade without /ALLUSERS or /CURRENTUSER keeps the previous install's
; mode, so it replaces that install instead of adding a second one.
UsePreviousPrivileges=yes
UsePreviousAppDir=yes
; Upgrades ask a running Topos to close. It is not restarted afterwards:
; Setup may run as administrator (Chocolatey, winget machine scope), and
; Topos must not come back elevated.
CloseApplications=yes
RestartApplications=no
; One Setup at a time, so two package managers cannot upgrade at once.
SetupMutex=ToposSetup,Global\ToposSetup
; The x86 build runs on any Windows that runs 32-bit programs and installs
; into Program Files (x86); the x64 build installs into Program Files.
ArchitecturesAllowed={#ArchAllowed}
#if TargetArch == "x64"
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0
UninstallDisplayIcon={app}\Topos.exe
UninstallDisplayName=Topos
VersionInfoVersion={#AppVersion}
LicenseFile={#SourceDir}\LICENSE-MIT
OutputDir={#OutputDir}
OutputBaseFilename=Topos-v{#AppVersion}-{#ArchSuffix}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Setup's own icon and the small image in the wizard's top corner come from
; the generated art. The small images are 32-bit BMPs with premultiplied
; alpha, one for each image-area size Inno Setup 6.6+ uses from 100 to
; 250 % scaling (58, 77, 97, 116, 124, 143, 159 px), so Setup shows each
; at its own size instead of stretching one. The large
; wizard image stays Inno Setup's built-in one.
SetupIconFile={#ArtDir}\Topos.ico
WizardSmallImageFile={#ArtDir}\wizard-small-58.bmp,{#ArtDir}\wizard-small-77.bmp,{#ArtDir}\wizard-small-97.bmp,{#ArtDir}\wizard-small-116.bmp,{#ArtDir}\wizard-small-124.bmp,{#ArtDir}\wizard-small-143.bmp,{#ArtDir}\wizard-small-159.bmp
WizardImageAlphaFormat=premultiplied

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\Topos.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Topos"; Filename: "{app}\Topos.exe"
Name: "{autodesktop}\Topos"; Filename: "{app}\Topos.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Topos.exe"; Description: "{cm:LaunchProgram,Topos}"; Flags: nowait postinstall skipifsilent

; Setup writes nothing outside the install folder, the shortcuts, and the
; Add/Remove Programs entry: no registry settings, no machine-wide
; defaults. Appearance and every other choice is a per-user setting the
; program keeps itself. Uninstalling leaves %LOCALAPPDATA%\Topos alone: it
; holds those settings (settings.txt) and each drive's last scan
; (*.toposcan), which a reinstall should keep.
