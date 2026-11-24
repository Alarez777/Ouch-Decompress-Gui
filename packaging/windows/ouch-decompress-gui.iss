#define AppName "Ouch Decompress"
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppPublisher "Alarez777"
#define AppURL "https://github.com/Alarez777/Ouch-Decompress-Gui"
#define AppExe "ouch-decompress-gui.exe"
#define ProgId "OuchDecompress.Archive"

; Emits the four registry entries needed for one extension.
#sub AssocExt
Root: HKCU; Subkey: "Software\Classes\.{#Ext}"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.{#Ext}\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".{#Ext}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".{#Ext}"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
#endsub

[Setup]
AppId={{281463a1-adee-4a5e-a687-0243b4b5aa43}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=Ouch-Decompress-Gui-Setup-{#AppVersion}
SetupIconFile=..\..\assets\ouch-decompress-gui.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
LicenseFile=..\..\LICENSE
VersionInfoVersion={#AppVersion}.0
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppName} installer
VersionInfoProductName={#AppName}
VersionInfoProductVersion={#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[CustomMessages]
english.Integration=Integration
spanish.Integration=Integracion
english.AssociateTask=Associate archive files with Ouch Decompress
spanish.AssociateTask=Asociar archivos comprimidos con Ouch Decompress

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "associate"; Description: "{cm:AssociateTask}"; GroupDescription: "{cm:Integration}"; Flags: checkedonce

[Files]
Source: "..\..\dist\windows\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\windows\ouch.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Registry]
; ProgID
Root: HKCU; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueName: ""; ValueData: "Ouch Decompress Archive"; Flags: uninsdeletekey; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Flags: uninsdeletekey; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Flags: uninsdeletekey; Tasks: associate

; Open with
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Flags: uninsdeletekey; Tasks: associate

; Default Apps registration
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "OuchDecompress"; ValueData: "Software\OuchDecompress\Capabilities"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Extract archives with ouch"; Flags: uninsdeletekey; Tasks: associate

; Extensions
#define Ext "tar"
AssocExt
#undef Ext
#define Ext "tgz"
AssocExt
#undef Ext
#define Ext "tbz"
AssocExt
#undef Ext
#define Ext "tbz2"
AssocExt
#undef Ext
#define Ext "tbz3"
AssocExt
#undef Ext
#define Ext "tlz4"
AssocExt
#undef Ext
#define Ext "txz"
AssocExt
#undef Ext
#define Ext "tlzma"
AssocExt
#undef Ext
#define Ext "tsz"
AssocExt
#undef Ext
#define Ext "tzst"
AssocExt
#undef Ext
#define Ext "tlz"
AssocExt
#undef Ext
#define Ext "cbt"
AssocExt
#undef Ext
#define Ext "zip"
AssocExt
#undef Ext
#define Ext "cbz"
AssocExt
#undef Ext
#define Ext "epub"
AssocExt
#undef Ext
#define Ext "7z"
AssocExt
#undef Ext
#define Ext "cb7"
AssocExt
#undef Ext
#define Ext "rar"
AssocExt
#undef Ext
#define Ext "cbr"
AssocExt
#undef Ext
#define Ext "gz"
AssocExt
#undef Ext
#define Ext "bz"
AssocExt
#undef Ext
#define Ext "bz2"
AssocExt
#undef Ext
#define Ext "bz3"
AssocExt
#undef Ext
#define Ext "xz"
AssocExt
#undef Ext
#define Ext "lzma"
AssocExt
#undef Ext
#define Ext "lz"
AssocExt
#undef Ext
#define Ext "lz4"
AssocExt
#undef Ext
#define Ext "sz"
AssocExt
#undef Ext
#define Ext "zst"
AssocExt
#undef Ext
#define Ext "br"
AssocExt
#undef Ext
