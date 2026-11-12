; Per-user installer for Jot (Inno Setup 6).
;
; No administrator rights: everything goes under the current user's profile
; (%LOCALAPPDATA%\Programs\Jot, the user's Start Menu, HKCU). Built by
; scripts/package.ps1; compile by hand with
;   ISCC /DAppVersion=1.5.0 /DStage=<staged folder> /DOutDir=<output> jot.iss
; Add /DTestBuild for the isolated test variant: a separate app id and folder,
; a test-profile marker beside jot.exe (data stays in <app>\profile), and a
; scratch registry key instead of the real Run key, so installing it can never
; start a test build with Windows or touch the real %LOCALAPPDATA%\Jot.
;
; Uninstalling keeps the user's data. It is deleted only after an explicit
; "Yes" (default "No") or, for a scripted uninstall, the /PURGEDATA switch.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef Stage
  #error Pass /DStage=<folder holding jot.exe and the licence files>
#endif
#ifndef OutDir
  #define OutDir "."
#endif

#ifdef TestBuild
  #define AppId "{{6C0B7E52-3F4A-4E7B-9C51-8E2A1D5F7B10}"
  #define AppName "Jot (test)"
  #define DirName "Jot-test"
  #define RunKey "Software\Jot-installer-test\Run"
  #define OutName "Jot-" + AppVersion + "-test-setup"
#else
  #define AppId "{{A3F1C9D2-7B64-4E0A-B8D5-2C9E4F61A7E3}"
  #define AppName "Jot"
  #define DirName "Jot"
  #define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
  #define OutName "Jot-" + AppVersion + "-setup"
#endif

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=tadikwa
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} {#AppVersion} Setup
AppPublisherURL=https://github.com/tadikwa/jot-gemini-transcribe-Windows
AppSupportURL=https://github.com/tadikwa/jot-gemini-transcribe-Windows/issues
AppUpdatesURL=https://github.com/tadikwa/jot-gemini-transcribe-Windows/releases
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=
DefaultDirName={autopf}\{#DirName}
DisableProgramGroupPage=yes
DisableDirPage=auto
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Jot holds this mutex while it runs (single_instance.rs): setup and uninstall
; ask the user to quit it instead of replacing a running executable.
AppMutex=Local\com.ammaar.jot.dictation
SetupIconFile={#SourcePath}\..\..\crates\jot\assets\jot.ico
UninstallDisplayIcon={app}\jot.exe
UninstallDisplayName={#AppName}
OutputDir={#OutDir}
OutputBaseFilename={#OutName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=auto

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
french.AutostartTask=Lancer Jot à l'ouverture de session Windows
english.AutostartTask=Start Jot when I sign in to Windows
french.ForeignRunEntry=Une entrée de démarrage nommée « Jot » existe déjà et ne lance pas jot.exe. Elle n'a pas été modifiée ; vous pourrez activer le démarrage automatique depuis les réglages de Jot.
english.ForeignRunEntry=A startup entry named "Jot" already exists and does not launch jot.exe. It was left untouched; you can turn on start with Windows from Jot's settings.
french.PurgeQuestion=Jot a été désinstallé. Vos données ont été conservées :%n%n%1%n(historique, dictionnaire, réglages, modèles locaux)%n%nVoulez-vous aussi les supprimer définitivement, ainsi que la clé API Gemini enregistrée ?%n%nCette suppression est irréversible. Les copies portables de Jot utilisent le même dossier.
english.PurgeQuestion=Jot was uninstalled. Your data was kept:%n%n%1%n(history, dictionary, settings, local models)%n%nDo you also want to delete it permanently, along with the saved Gemini API key?%n%nThis cannot be undone. Portable copies of Jot use the same folder.
#ifdef TestBuild
french.PurgeQuestionTest=Version de test désinstallée. Supprimer aussi son profil isolé ?%n%n%1
english.PurgeQuestionTest=Test build uninstalled. Also delete its isolated profile?%n%n%1
#endif

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Stage}\jot.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\NOTICE-FR.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
#ifdef TestBuild
Source: "{#Stage}\jot-test-profile"; DestDir: "{app}"; Flags: ignoreversion
#endif

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\jot.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\jot.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\jot.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  RunKey = '{#RunKey}';
  RunName = 'Jot';
  CredentialTarget = 'gemini-api-key.com.ammaar.jot';

function JotExe(): String;
begin
  Result := ExpandConstant('{app}\jot.exe');
end;

{ Same rule as autostart.rs: only a value that launches some jot.exe is ours. }
function LaunchesJot(Command: String): Boolean;
var
  Lower: String;
begin
  Lower := Lowercase(Command);
  Result := (Pos('\jot.exe"', Lower) > 0) or
    ((Length(Lower) >= 8) and (Copy(Lower, Length(Lower) - 7, 8) = '\jot.exe'));
end;

function HasParam(Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

function DataDir(): String;
begin
#ifdef TestBuild
  Result := ExpandConstant('{app}\profile');
#else
  Result := ExpandConstant('{localappdata}\Jot');
#endif
end;

procedure EnableAutostart();
var
  Existing: String;
begin
  if RegQueryStringValue(HKCU, RunKey, RunName, Existing) and (Existing <> '') and
     not LaunchesJot(Existing) then
  begin
    Log('Run\Jot belongs to another program; left untouched: ' + Existing);
    if not WizardSilent then
      MsgBox(CustomMessage('ForeignRunEntry'), mbInformation, MB_OK);
    Exit;
  end;
  { The exact command Jot itself writes, so Settings shows it as enabled. }
  if RegWriteStringValue(HKCU, RunKey, RunName, '"' + JotExe() + '" --background') then
    Log('Run\Jot set to ' + JotExe())
  else
    Log('Could not write Run\Jot');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  { Left unticked, an existing entry is kept: the user may have enabled it in Jot. }
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('autostart') then
    EnableAutostart();
end;

procedure RemoveOurAutostart();
var
  Existing: String;
begin
  if RegQueryStringValue(HKCU, RunKey, RunName, Existing) and
     (Pos(Lowercase(JotExe()), Lowercase(Existing)) > 0) then
  begin
    RegDeleteValue(HKCU, RunKey, RunName);
    Log('Removed Run\Jot pointing at this install');
  end;
end;

procedure PurgeData();
var
  Code: Integer;
begin
  if DirExists(DataDir()) then
  begin
    if DelTree(DataDir(), True, True, True) then
      Log('Deleted ' + DataDir())
    else
      Log('Could not fully delete ' + DataDir());
  end;
#ifndef TestBuild
  { The test build's key has a per-folder hashed name that setup cannot compute. }
  if Exec(ExpandConstant('{sys}\cmdkey.exe'), '/delete:' + CredentialTarget, '',
          SW_HIDE, ewWaitUntilTerminated, Code) then
    Log('cmdkey /delete exit code ' + IntToStr(Code));
#endif
end;

function WantsPurge(): Boolean;
var
  Question: String;
begin
  if UninstallSilent then
  begin
    Result := HasParam('/PURGEDATA');
    Exit;
  end;
#ifdef TestBuild
  Question := FmtMessage(CustomMessage('PurgeQuestionTest'), [DataDir()]);
#else
  Question := FmtMessage(CustomMessage('PurgeQuestion'), [DataDir()]);
#endif
  Result := MsgBox(Question, mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveOurAutostart();
  if (CurUninstallStep = usPostUninstall) and WantsPurge() then
    PurgeData();
end;
