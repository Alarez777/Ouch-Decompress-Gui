#define AppName "Ouch Decompress"
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppPublisher "Alarez777"
#define AppURL "https://github.com/Alarez777/Ouch-Decompress-Gui"
#define AppExe "ouch-decompress-gui.exe"
#define ProgId "OuchDecompress.Archive"


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

Root: HKCU; Subkey: "Software\Classes\.tar"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tar\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tar"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tar"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tgz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tgz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tgz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tgz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tbz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tbz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tbz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tbz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tbz2"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tbz2\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tbz2"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tbz2"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tbz3"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tbz3\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tbz3"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tbz3"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tlz4"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tlz4\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tlz4"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tlz4"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.txz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.txz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".txz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".txz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tlzma"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tlzma\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tlzma"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tlzma"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tsz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tsz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tsz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tsz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tzst"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tzst\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tzst"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tzst"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tlz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.tlz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tlz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".tlz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cbt"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cbt\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".cbt"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".cbt"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.zip"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.zip\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".zip"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".zip"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cbz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cbz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".cbz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".cbz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.epub"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.epub\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".epub"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".epub"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.7z"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.7z\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".7z"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".7z"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cb7"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cb7\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".cb7"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".cb7"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.rar"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.rar\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".rar"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".rar"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cbr"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.cbr\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".cbr"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".cbr"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.gz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.gz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".gz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".gz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.bz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.bz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".bz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".bz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.bz2"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.bz2\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".bz2"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".bz2"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.bz3"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.bz3\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".bz3"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".bz3"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.xz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.xz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".xz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".xz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.lzma"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.lzma\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".lzma"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".lzma"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.lz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.lz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".lz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".lz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.lz4"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.lz4\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".lz4"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".lz4"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.sz"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.sz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".sz"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".sz"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.zst"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.zst\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".zst"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".zst"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.br"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\.br\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".br"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\OuchDecompress\Capabilities\FileAssociations"; ValueType: string; ValueName: ".br"; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: associate
