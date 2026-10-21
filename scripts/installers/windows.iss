; Values are passed as ISCC /D parameters by runtime_installers.py.
[Setup]
AppId={{C34EF11F-6783-4565-B367-F8A61724E01E}
AppName=AhaKey Runtime
AppVersion={#AppVersion}
AppPublisher=AhaKeyAI
AppPublisherURL=https://github.com/AhakeyAI/ahakey-runtime-rust
DefaultDirName={localappdata}\Programs\AhaKey Runtime
DefaultGroupName=AhaKey Runtime
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename={#FileName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
LicenseFile={#SourceDir}\LICENSE
UninstallDisplayName=AhaKey Runtime
CloseApplications=yes
RestartApplications=no

[InstallDelete]
Type: files; Name: "{app}\BUILD-INFO.json"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\AhaKey Runtime"; Filename: "{app}\ahakey-runtime-rust.exe"; WorkingDir: "{app}"
Name: "{group}\使用说明 - Instructions"; Filename: "{app}\README.txt"

[Run]
Filename: "{app}\ahakey-runtime-rust.exe"; Description: "启动 AhaKey Runtime / Start AhaKey Runtime"; Flags: nowait postinstall skipifsilent unchecked
