; Inno Setup script for the KERN installer (Windows 11 x64).
;   iscc installer\KERN.iss            (after scripts\build-windows.ps1)
; Settings and chat history live in %APPDATA%\kern and are kept on update and uninstall.

#define AppVersion "0.1.3"

[Setup]
AppId={{7C1B2E4A-5F0D-4C8E-9C3B-2A1E6D8F4B10}
AppName=KERN
AppVersion={#AppVersion}
AppPublisher=KERN
DefaultDirName={autopf}\KERN
DefaultGroupName=KERN
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist\windows
OutputBaseFilename=KERN-windows-x64-setup
SetupIconFile=..\assets\windows\kern.ico
UninstallDisplayIcon={app}\KERN.exe
; Makes Explorer reload icons when setup finishes (SHCNE_ASSOCCHANGED). Without it
; an update keeps showing the previous icon cached for the same KERN.exe path.
ChangesAssociations=yes
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\dist\windows\KERN\KERN.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The icon comes from KERN.exe itself: resource ID 1 (assets/windows/kern.rc), named
; as -1. Explorer caches icons by "path,index"; v0.1.0 shortcuts used "KERN.exe,0",
; so naming the resource ID gives a new key and Explorer reads the new icon after an
; update instead of the cached old one. It is the same icon as index 0.
Name: "{autoprograms}\KERN"; Filename: "{app}\KERN.exe"; IconFilename: "{app}\KERN.exe"; IconIndex: -1
Name: "{autodesktop}\KERN"; Filename: "{app}\KERN.exe"; IconFilename: "{app}\KERN.exe"; IconIndex: -1; Tasks: desktopicon

[Run]
; After an update over an older KERN, Explorer may keep the previous icon for the
; same path in its icon cache: this asks it to reload icons.
Filename: "{sys}\ie4uinit.exe"; Parameters: "-show"; Flags: runhidden skipifdoesntexist
Filename: "{app}\KERN.exe"; Description: "{cm:LaunchProgram,KERN}"; Flags: nowait postinstall skipifsilent
; An in-app update runs this installer silently: KERN starts again on the new version.
Filename: "{app}\KERN.exe"; Flags: nowait runasoriginaluser; Check: IsKernUpdate

[Code]
// Tells Explorer that these files changed, so it redraws their icons from the
// new KERN.exe instead of its cache (SHCNE_UPDATEITEM, SHCNF_PATHW).
procedure SHChangeNotify(EventId: Integer; Flags: Cardinal; Item1, Item2: String);
  external 'SHChangeNotify@shell32.dll stdcall';

// Started by KERN with /KERNUPDATE=1 (Help > Check for Updates > Restart to Update).
function IsKernUpdate: Boolean;
begin
  Result := ExpandConstant('{param:KERNUPDATE|0}') = '1';
end;

// KERN holds the "KernAppRunning" mutex until it has quit. An update waits for
// that (up to a minute) before replacing files.
function InitializeSetup: Boolean;
var
  Waited: Integer;
begin
  Result := True;
  if IsKernUpdate then
  begin
    Waited := 0;
    while CheckForMutexes('KernAppRunning') and (Waited < 600) do
    begin
      Sleep(100);
      Waited := Waited + 1;
    end;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    SHChangeNotify($2000, $0005, ExpandConstant('{app}\KERN.exe'), '');
    SHChangeNotify($2000, $0005, ExpandConstant('{autodesktop}\KERN.lnk'), '');
    SHChangeNotify($2000, $0005, ExpandConstant('{autoprograms}\KERN.lnk'), '');
  end;
end;
