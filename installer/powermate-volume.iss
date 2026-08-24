#define MyAppName "PowerMate Volume"
#define MyAppPublisher "Paul Wicks"
#define MyAppExeName "powermate-volume.exe"
#define MyAppSourceExe "..\target\release\" + MyAppExeName
; build.rs stamps the exe's version resource from Cargo.toml, so reading it
; back here keeps Cargo.toml the single source of truth -- there's no second
; version to remember to bump at release time.
#define MyAppVersion GetFileVersion(MyAppSourceExe)

[Setup]
; Keep this GUID fixed across versions so Inno treats new builds as
; upgrades of the same install rather than separate side-by-side installs.
AppId={{F32A52FF-B73D-457E-9419-7D974C44CEF8}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=output
OutputBaseFilename=PowerMateVolumeSetup
Compression=lzma
SolidCompression=yes
WizardStyle=modern
PrivilegesRequiredOverridesAllowed=dialog
; The exe is a 64-bit build; without this Inno defaults to 32-bit mode and
; {autopf} resolves to "Program Files (x86)" instead of "Program Files".
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Close the running tray app (if any) before overwriting its exe, and
; offer to relaunch it afterwards.
CloseApplications=yes
RestartApplications=yes

[Files]
; build.rs stamps a version resource from Cargo.toml, but dev rebuilds don't
; bump that version -- so without ignoreversion, Inno would treat a freshly
; built exe as "same version, skip" and silently install nothing.
Source: "{#MyAppSourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"

[Tasks]
Name: "autostart"; Description: "Start {#MyAppName} automatically when Windows starts"; GroupDescription: "Additional options:"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

[Code]
// This registry entry is the same one the app itself reads/writes from its
// "Start with Windows" tray menu toggle, so both stay in sync.
const
  AutostartKeyPath = 'Software\Microsoft\Windows\CurrentVersion\Run';
  AutostartValueName = 'PowerMateVolume';

var
  IsUpgradeInstall: Boolean;

// Checked once at startup, before anything is written, so it reflects
// whether an install already existed rather than what this run just did.
function InitializeSetup: Boolean;
var
  Value: string;
  UninstallKey: string;
begin
  // Must match [Setup] AppId above.
  UninstallKey := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{F32A52FF-B73D-457E-9419-7D974C44CEF8}_is1';
  IsUpgradeInstall :=
    RegQueryStringValue(HKLM, UninstallKey, 'UninstallString', Value) or
    RegQueryStringValue(HKCU, UninstallKey, 'UninstallString', Value);
  Result := True;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  // Only set this on a fresh install. On an upgrade, leave it alone —
  // the user may have turned it off via the tray since installing, and a
  // routine update shouldn't silently re-enable it behind their back.
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('autostart') and not IsUpgradeInstall then
    RegWriteStringValue(HKCU, AutostartKeyPath, AutostartValueName,
      '"' + ExpandConstant('{app}\{#MyAppExeName}') + '"');
end;

// Clean up the autostart entry on uninstall so a removed exe doesn't leave
// a dangling Run key behind, whether it was set by the installer above or
// by the app's own tray toggle later.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKCU, AutostartKeyPath, AutostartValueName);
end;
