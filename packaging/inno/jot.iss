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
AppPublisherURL=https://github.com/tadikwa/JOT
AppSupportURL=https://github.com/tadikwa/JOT/issues
AppUpdatesURL=https://github.com/tadikwa/JOT/releases
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=
DefaultDirName={autopf}\{#DirName}
DisableProgramGroupPage=yes
DisableDirPage=auto
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Jot holds this mutex while it runs (single_instance.rs): setup and uninstall
; ask the user to quit it instead of replacing a running executable. Setup first
; asks this folder's Jot to close itself (InitializeSetup, below); the mutex stays
; as the guarantee that a running jot.exe is never replaced.
AppMutex=Local\com.ammaar.jot.dictation
; Never let Restart Manager close or restart Jot: only Jot decides to quit.
CloseApplications=no
RestartApplications=no
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
french.JotRunningAsk=Jot est encore ouvert. Le fermer pour lancer l'installation ?%n%nJot se fermera normalement ; vos réglages, votre historique, votre dictionnaire et vos modèles sont conservés.
english.JotRunningAsk=Jot is still open. Close it to start the installation?%n%nJot will close normally; your settings, history, dictionary and models are kept.
french.JotBusy=Jot ne s'est pas fermé : une dictée, un téléchargement ou un chargement de modèle est sans doute en cours.%n%nLaissez-le se terminer puis cliquez sur Réessayer, ou fermez Jot vous-même (clic droit sur son icône près de l'horloge > Quitter Jot).
english.JotBusy=Jot did not close: a dictation, a download or a model loading is probably in progress.%n%nLet it finish and click Retry, or close Jot yourself (right-click its icon near the clock > Quit Jot).
french.JotManualClose=Jot est en cours d'exécution et ne peut pas être fermé par l'installation (version antérieure à 1.7, ou copie lancée depuis un autre dossier).%n%nFermez-le vous-même (clic droit sur son icône près de l'horloge > Quitter Jot), puis cliquez sur Réessayer.
english.JotManualClose=Jot is running and cannot be closed by setup (a version before 1.7, or a copy started from another folder).%n%nClose it yourself (right-click its icon near the clock > Quit Jot), then click Retry.
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

  DictationMutex = 'Local\com.ammaar.jot.dictation';
  EVENT_MODIFY_STATE = $0002;

function OpenEventW(Access: Cardinal; Inherit: BOOL; Name: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function SetEvent(Event: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

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

{ Same recipe as jot_core::update::quit_event_name: one event per folder, so
  setup only ever asks the copy it is about to replace. }
function QuitEventName(Dir: String): String;
begin
  Result := 'Local\com.ammaar.jot.quit.' +
    Copy(GetSHA256OfUnicodeString(Lowercase(RemoveBackslash(Dir))), 1, 16);
end;

{ True when the Jot installed here listens for requests and was sent one.
  Jot 1.6 and copies run from elsewhere have no listener: False. }
function AskJotToClose(): Boolean;
var
  Dir: String;
  Event: THandle;
begin
  Result := False;
  if not RegQueryStringValue(HKCU,
       'Software\Microsoft\Windows\CurrentVersion\Uninstall\' +
       Copy('{#AppId}', 2, MaxInt) + '_is1', 'InstallLocation', Dir) or (Dir = '') then
    Exit;
  Event := OpenEventW(EVENT_MODIFY_STATE, False, QuitEventName(Dir));
  if Event = 0 then
    Exit;
  Result := SetEvent(Event);
  CloseHandle(Event);
  Log('Asked the Jot in ' + Dir + ' to close');
end;

function WaitForJotToClose(Seconds: Integer): Boolean;
var
  I: Integer;
begin
  for I := 1 to Seconds * 4 do
  begin
    if not CheckForMutexes(DictationMutex) then
    begin
      Result := True;
      Exit;
    end;
    Sleep(250);
  end;
  Result := not CheckForMutexes(DictationMutex);
end;

{ Runs before the AppMutex check. Jot is only ever asked to close, never
  killed: if it stays open, the user decides. }
function InitializeSetup(): Boolean;
var
  Asked: Boolean;
  Prompt: String;
begin
  Result := True;
  if not CheckForMutexes(DictationMutex) then
    Exit;
  { Started by Jot after the user confirmed there: it is already closing. }
  if HasParam('/JOTWAIT') and WaitForJotToClose(20) then
    Exit;
  if SuppressibleMsgBox(CustomMessage('JotRunningAsk'), mbConfirmation, MB_OKCANCEL,
       IDOK) <> IDOK then
  begin
    Result := False;
    Exit;
  end;
  repeat
    Asked := AskJotToClose();
    if Asked and WaitForJotToClose(15) then
      Exit;
    if Asked then
      Prompt := CustomMessage('JotBusy')
    else
      Prompt := CustomMessage('JotManualClose');
    if SuppressibleMsgBox(Prompt, mbError, MB_RETRYCANCEL, IDCANCEL) <> IDRETRY then
    begin
      Log('Jot still running; setup cancelled');
      Result := False;
      Exit;
    end;
  until not CheckForMutexes(DictationMutex);
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
