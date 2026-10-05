; Inno Setup Script per nextm
; Monitor di sistema leggerissimo per Windows 11 (Pure Win32 GDI, zero runtime DLL)

#define MyAppName "nextm"
#ifndef MyAppVersion
#define MyAppVersion "0.1.2"
#endif
#define MyAppPublisher "Lorenzo Pompili"
#define MyAppURL "https://github.com/lorenzopompili/nextm"
#define MyAppExeName "nextm.exe"

[Setup]
; Identificatore univoco GUID per l'applicazione
AppId={{8B1D4E92-3C4F-49E0-9E14-7A9B31E9F123}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} v{#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}

; Modalità di installazione:
; "lowest": permette installazione per l'utente corrente in %LocalAppData%\Programs\nextm (senza UAC)
; Se eseguito come admin o se l'utente richiede "Per tutti gli utenti", installa in C:\Program Files\nextm
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ShowLanguageDialog=yes

; Output dell'installer
OutputDir=..\target\dist
OutputBaseFilename=nextm-setup-v{#MyAppVersion}
SetupIconFile=..\logo\ICO\hal_ecg_app.ico
UninstallDisplayIcon={app}\{#MyAppExeName}

; Compressione massima LZMA2
Compression=lzma2/max
SolidCompression=yes

; Stile interfaccia moderna Windows 11
WizardStyle=modern

; Supporto 64-bit puro
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; Gestione chiusura processi attivi durante installazione/aggiornamento
CloseApplications=force
CloseApplicationsFilter={#MyAppExeName}

; Informazioni di disinstallazione pulite in Impostazioni Windows (App installate)
UninstallDisplayName={#MyAppName}
VersionInfoVersion={#MyAppVersion}
VersionInfoDescription=nextm Setup
VersionInfoProductName={#MyAppName}

[Languages]
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
italian.AutoStartDesc=Avvia {#MyAppName} automaticamente all'accesso a Windows
italian.AutoStartGroup=Opzioni di avvio:
english.AutoStartDesc=Start {#MyAppName} automatically on Windows logon
english.AutoStartGroup=Startup options:

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "{cm:AutoStartDesc}"; GroupDescription: "{cm:AutoStartGroup}"; Flags: checkedonce

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Menu Start (Windows)
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"
; Icona opzionale Desktop
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
; Avvio automatico se selezionato nella task di installazione
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\{#MyAppExeName}"""; Tasks: autostart; Flags: uninsdeletevalue
; Pulizia registro impostazioni utente alla disinstallazione
Root: HKCU; Subkey: "Software\{#MyAppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"; ValueName: "{#MyAppName}"; Flags: uninsdeletevalue

[Run]
; Esecuzione al termine dell'installazione
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Chiudi l'applicazione se in esecuzione prima di disinstallare
Filename: "taskkill.exe"; Parameters: "/F /IM {#MyAppExeName}"; Flags: runhidden; RunOnceId: "CloseNextm"
; Elimina l'eventuale attività pianificata come Amministratore (Admin Task)
Filename: "schtasks.exe"; Parameters: "/delete /tn ""{#MyAppName}"" /f"; Flags: runhidden; RunOnceId: "DelSchtaskNextm"

[UninstallDelete]
; Pulizia al millimetro di ogni file generato (es. nextm.ini in caso di portable locale o file temporanei)
Type: files; Name: "{app}\nextm.ini"
Type: filesandordirs; Name: "{app}\*"
Type: dirifempty; Name: "{app}"

[Code]
// Pulizia completa al termine della disinstallazione
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // Rimuove qualsiasi residuo da HKCU\Software\nextm
    RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, 'Software\nextm');
  end;
end;
