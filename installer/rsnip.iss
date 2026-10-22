; The Windows installer, built with Inno Setup 6 after `cargo build --release`:
;
;   iscc installer\rsnip.iss
;
; It installs for the current user (no admin prompt) into %LOCALAPPDATA%\Programs\RSnip, and is
; written to target\installer. The version is read from the built exe. /DBin=<dir> packages an exe
; from another folder (CI uses the debug build).

#ifndef Bin
  #define Bin "..\target\release"
#endif
#define AppVersion GetFileProductVersion(AddBackslash(SourcePath) + Bin + "\rsnip.exe")
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"

[Setup]
AppId={{E08792D2-E429-498E-BEB1-A3BA1E251749}
AppName=RSnip
AppVersion={#AppVersion}
AppPublisher=RSnip contributors
AppPublisherURL=https://github.com/dhiorns15/rust-screenshot
DefaultDirName={autopf}\RSnip
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 or later (Windows 11 is also 10.0); setup refuses to run on older versions.
MinVersion=10.0
SetupIconFile={#Bin}\rsnip.ico
UninstallDisplayIcon={app}\rsnip.exe
UninstallDisplayName=RSnip
VersionInfoVersion={#AppVersion}
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
; RSnip is closed by CloseRSnip below; this catches anything still holding the exe.
CloseApplications=yes
RestartApplications=no
OutputDir=..\target\installer
OutputBaseFilename=rsnip-v{#AppVersion}-windows-x64-setup

[Tasks]
Name: "startup"; Description: "Start RSnip when I sign in to Windows"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Bin}\rsnip.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\RSnip"; Filename: "{app}\rsnip.exe"
Name: "{autoprograms}\RSnip Library"; Filename: "{app}\rsnip.exe"; Parameters: "--gallery"
Name: "{autodesktop}\RSnip"; Filename: "{app}\rsnip.exe"; Tasks: desktopicon

[Registry]
; The same entry as the tray menu's "Start with Windows", so the two stay in step.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "RSnip"; ValueData: """{app}\rsnip.exe"""; Tasks: startup

[Run]
Filename: "{app}\rsnip.exe"; Description: "Start RSnip now"; Flags: nowait postinstall skipifsilent
; RSnip's updater runs setup silently with /relaunch=yes, to start the new version afterwards.
Filename: "{app}\rsnip.exe"; Flags: nowait; Check: Relaunch

[Code]
const
  WM_CLOSE = $0010;

// Asks each window of one kind to close, for up to two seconds.
procedure CloseWindows(const ClassName, Title: String);
var
  Wnd: HWND;
  I: Integer;
begin
  for I := 1 to 20 do
  begin
    if ClassName <> '' then
      Wnd := FindWindowByClassName(ClassName)
    else
      Wnd := FindWindowByWindowName(Title);
    if Wnd = 0 then
      Exit;
    PostMessage(Wnd, WM_CLOSE, 0, 0);
    Sleep(100);
  end;
end;

// Closes a running RSnip so its exe can be replaced or removed: the tray app, pinned images, and
// the library (which first asks about unsaved edits, and stays open if they are kept).
procedure CloseRSnip;
begin
  CloseWindows('RSnip.Main', '');
  CloseWindows('RSnip.Pin', '');
  CloseWindows('', 'RSnip Library');
  // Let the processes exit after their windows have gone.
  Sleep(500);
end;

function Relaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|no}') = 'yes');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseRSnip;
  Result := '';
end;

function InitializeUninstall: Boolean;
begin
  CloseRSnip;
  Result := True;
end;

// Removes the start-at-sign-in entry, whether setup or the tray menu added it, if it points here.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Command: String;
begin
  if (CurUninstallStep = usUninstall)
    and RegQueryStringValue(HKCU, '{#RunKey}', 'RSnip', Command)
    and (Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Command)) > 0) then
    RegDeleteValue(HKCU, '{#RunKey}', 'RSnip');
end;
