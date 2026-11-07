; Inno Setup script for the KERN installer (Windows 11 x64).
;   iscc installer\KERN.iss            (after scripts\build-windows.ps1)
; Settings and chat history live in %APPDATA%\kern and are kept on update and uninstall.

#define AppVersion "0.1.0"

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
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\dist\windows\KERN\KERN.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The icon comes from KERN.exe itself (resource 1, assets/windows/kern.ico).
Name: "{autoprograms}\KERN"; Filename: "{app}\KERN.exe"; IconFilename: "{app}\KERN.exe"; IconIndex: 0
Name: "{autodesktop}\KERN"; Filename: "{app}\KERN.exe"; IconFilename: "{app}\KERN.exe"; IconIndex: 0; Tasks: desktopicon

[Run]
; After an update over an older KERN, Explorer may keep the previous icon for the
; same path in its icon cache: this asks it to reload icons.
Filename: "{sys}\ie4uinit.exe"; Parameters: "-show"; Flags: runhidden skipifdoesntexist
Filename: "{app}\KERN.exe"; Description: "{cm:LaunchProgram,KERN}"; Flags: nowait postinstall skipifsilent
