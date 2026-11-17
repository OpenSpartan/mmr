; The mmr installer, for Inno Setup 7 (https://jrsoftware.org/isinfo.php).
;
;   cargo build --release
;   ISCC /DAppVersion=0.0.1 installer\mmr.iss
;
; The setup goes to target\installer. It installs for the current user, so it never
; asks for admin. Running a newer setup over an installed mmr updates it in place:
; same folder, same choices, and the certificate and saved matches stay.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define Repo "https://github.com/OpenSpartan/mmr"
; Where mmr keeps its certificate and saved matches (store::default_dir).
#define Store "{localappdata}\hi-mmr"

[Setup]
; Never change AppId: it's how a newer setup finds the installed mmr.
AppId={{FA1023C4-3673-4662-B7D8-F1752828E64A}
AppName=mmr
AppVersion={#AppVersion}
AppVerName=mmr {#AppVersion}
AppPublisher=OpenSpartan
AppPublisherURL={#Repo}
AppSupportURL={#Repo}/issues
AppUpdatesURL={#Repo}/releases
AppCopyright=Copyright (c) 2026 OpenSpartan
VersionInfoVersion={#AppVersion}
VersionInfoDescription=mmr setup
; `mmr watch` holds this while it runs, so setup and uninstall ask for it to be closed.
AppMutex=OpenSpartan.mmr.watch
SetupMutex=OpenSpartan.mmr.setup
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
PrivilegesRequired=lowest
DefaultDirName={autopf}\mmr
DisableProgramGroupPage=yes
DisableWelcomePage=no
DisableReadyPage=yes
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=mmr
UninstallDisplayIcon={app}\mmr.exe
SetupIconFile=..\assets\icon.ico
; Follows the Windows light or dark mode, with the same images in both.
WizardStyle=modern dynamic
#define WizardImages "images\wizard-202.png,images\wizard-269.png,images\wizard-336.png,images\wizard-430.png,images\wizard-534.png"
#define SmallImages "images\small-58.png,images\small-77.png,images\small-97.png,images\small-124.png,images\small-159.png"
WizardImageFile={#WizardImages}
WizardImageFileDynamicDark={#WizardImages}
WizardSmallImageFile={#SmallImages}
WizardSmallImageFileDynamicDark={#SmallImages}
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\target\installer
OutputBaseFilename=mmr-setup-{#AppVersion}

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Messages]
WelcomeLabel1=Welcome to mmr
WelcomeLabel2=This installs [name/ver] for your account. mmr shows the hidden matchmaking rating of every player in your Halo Infinite lobbies.%n%nmmr is experimental: use it at your own risk. It isn't endorsed or supported by Microsoft or Halo Studios.
WizardSelectTasks=Set up mmr
SelectTasksDesc=Choose what Setup does as it installs mmr.
SelectTasksLabel2=Choose what you'd like, then click Install.
FinishedHeadingLabel=mmr is ready
FinishedLabel=Start mmr from the Start menu, or run mmr watch in a terminal, before you start Halo Infinite. It has to be running before the game connects to its lobby.
FinishedLabelNoIcons=Run mmr watch in a terminal before you start Halo Infinite. It has to be running before the game connects to its lobby.

[CustomMessages]
WelcomeUpdate=This updates mmr %1 to %2. Your certificate, settings and saved matches stay as they are.
Experimental=mmr is experimental: use it at your own risk. It isn't endorsed or supported by Microsoft or Halo Studios.
TaskCert=Make and trust mmr's local certificate now (Windows asks you to confirm)
TaskPath=Add mmr to my PATH, to run it from any terminal
RunWatch=Start mmr now
StatusCert=Making mmr's local certificate
StatusRemoveCert=Removing mmr's local certificate
ShortcutComment=Shows the hidden MMR of every player in your Halo Infinite lobby
UninstallMatches=Delete your saved matches too? They're in %1.

[Tasks]
Name: "cert"; Description: "{cm:TaskCert}"; Check: NoRootYet
Name: "path"; Description: "{cm:TaskPath}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\mmr.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\mmr"; Filename: "{app}\mmr.exe"; Parameters: "watch"; WorkingDir: "{app}"; Comment: "{cm:ShortcutComment}"
Name: "{autodesktop}\mmr"; Filename: "{app}\mmr.exe"; Parameters: "watch"; WorkingDir: "{app}"; Comment: "{cm:ShortcutComment}"; Tasks: desktopicon

[Run]
Filename: "{app}\mmr.exe"; Parameters: "cert new"; StatusMsg: "{cm:StatusCert}"; Flags: runhidden waituntilterminated; Tasks: cert
Filename: "{app}\mmr.exe"; Parameters: "watch"; WorkingDir: "{app}"; Description: "{cm:RunWatch}"; Flags: postinstall nowait skipifsilent; Check: HasRoot

[UninstallRun]
; Removes the root from the Root store, and restores the system proxy if a crashed
; `mmr watch` left it pointing at mmr.
Filename: "{app}\mmr.exe"; Parameters: "cert remove"; StatusMsg: "{cm:StatusRemoveCert}"; RunOnceId: "RemoveRoot"; Flags: runhidden waituntilterminated

[Code]
const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{FA1023C4-3673-4662-B7D8-F1752828E64A}_is1';

function HasRoot: Boolean;
begin
  Result := FileExists(ExpandConstant('{#Store}\cert\root.key'));
end;

{ Offered only while there's no certificate, since making another brings up the Windows prompt again. }
function NoRootYet: Boolean;
begin
  Result := not HasRoot;
end;

{ The version installed now, or '' on a first install. }
function Installed: String;
begin
  if not RegQueryStringValue(HKCU, UninstallKey, 'DisplayVersion', Result) then
    Result := '';
end;

procedure InitializeWizard;
var
  Before: String;
begin
  Before := Installed;
  if Before <> '' then
    WizardForm.WelcomeLabel2.Caption :=
      FmtMessage(CustomMessage('WelcomeUpdate'), [Before, '{#AppVersion}']) + #13#10#13#10 + CustomMessage('Experimental');
end;

{ With no Ready page, the tasks page is the last one before installing. }
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpSelectTasks then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall)
  else if CurPageID = wpFinished then
    WizardForm.NextButton.Caption := SetupMessage(msgButtonFinish)
  else
    WizardForm.NextButton.Caption := SetupMessage(msgButtonNext);
end;

{ Whether Paths, the user's PATH from HKCU\Environment, lists Dir. Case doesn't matter. }
function PathHas(Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Paths) then
    Paths := '';
  if PathHas(Paths, Dir) then
    Exit;
  if (Paths <> '') and (Paths[Length(Paths)] <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths: String;
  At: Integer;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Paths) then
    Exit;
  Paths := ';' + Paths + ';';
  At := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if At = 0 then
    Exit;
  Delete(Paths, At, Length(Dir) + 1);
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Copy(Paths, 2, Length(Paths) - 2));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    if WizardIsTaskSelected('path') then
      AddToPath(ExpandConstant('{app}'))
    else
      RemoveFromPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Matches: String;
begin
  if CurUninstallStep = usUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
  if CurUninstallStep = usPostUninstall then
  begin
    Matches := ExpandConstant('{#Store}\matches');
    if DirExists(Matches) and
      (SuppressibleMsgBox(FmtMessage(CustomMessage('UninstallMatches'), [Matches]),
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES) then
    begin
      DelTree(Matches, True, True, True);
      RemoveDir(ExpandConstant('{#Store}'));
    end;
  end;
end;
