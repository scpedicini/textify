#ifndef TextifyVersion
  #define TextifyVersion "0.0.0"
#endif
#ifndef SourceExe
  #error SourceExe must point to the release Textify.exe
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{03EF16C2-96DC-4F2F-BA6C-388B12484E7E}
AppName=Textify
AppVersion={#TextifyVersion}
AppPublisher=Textify
AppPublisherURL=https://github.com/scpedicini/textify
DefaultDirName={localappdata}\Programs\Textify
DefaultGroupName=Textify
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\Textify.exe
OutputDir={#OutputDir}
OutputBaseFilename=textify-{#TextifyVersion}-windows-x64-setup
SetupIconFile=Textify.ico

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Textify"; Filename: "{app}\Textify.exe"
Name: "{autodesktop}\Textify"; Filename: "{app}\Textify.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Registry]
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Textify"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\DefaultIcon"; ValueType: string; ValueData: "{app}\Textify.exe,0"
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\shell\open\command"; ValueType: string; ValueData: """{app}\Textify.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".txt"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".md"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".json"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".xml"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".html"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".css"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\Textify.exe\SupportedTypes"; ValueType: string; ValueName: ".rs"; ValueData: ""

[Run]
Filename: "{app}\Textify.exe"; Description: "Launch Textify"; Flags: nowait postinstall skipifsilent
