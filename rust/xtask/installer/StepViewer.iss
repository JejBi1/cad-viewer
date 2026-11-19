; Inno Setup script – CAD prohlížeč 2.0 (Rust), STEP / IGES / STL / DXF / DWG
;
; `cargo xtask dist` compiles this file (ISCC from "%LOCALAPPDATA%\Programs\Inno Setup 6", no admin rights needed) and
; passes the build-specific values as defines:
;   /DAppVersion=2.0.0  /DStageDir=<folder with StepViewer.exe, StepThumb.dll, licenses, third_party>
;   /DRepoDir=<repository root>  [/DNoDwg=1: base installer without the DWG converter, offered as a separate add-on]
; By hand:  ISCC.exe /O<output folder> /DStageDir=<stage> /DRepoDir=<repo> StepViewer.iss
;
; Same AppId as 1.0 .. 1.9: the installer upgrades an existing installation in place. The 1.9 program lived in a
; PyInstaller folder (_internal\) that is deleted on upgrade; 2.0 is one executable.

#ifndef AppVersion
  #define AppVersion "2.0.0"
#endif
#ifndef StageDir
  #error StageDir is not defined (pass /DStageDir=<folder> or run cargo xtask dist)
#endif
#ifndef RepoDir
  #define RepoDir "..\..\.."
#endif
#ifndef OutDir
  #define OutDir GetEnv("TEMP")
#endif
#ifdef NoDwg
  #define Suffix "_nodwg"
#else
  #define Suffix ""
#endif

#define AppName "CAD prohlížeč"
#define AppExe "StepViewer.exe"
#define ProgId "StepViewer.Model"
#define ProgId2D "StepViewer.Drawing"
#define ThumbClsid "{{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}"
#define ThumbIid "{{e357fccd-a995-4576-b01f-234630154e96}"

[Setup]
; same AppId as 1.0 (STEP prohlížeč) -> installs as an upgrade
AppId={{6E3B2F0C-6C55-4B7E-9A4D-2F1D8C5A7B10}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
VersionInfoVersion={#AppVersion}
AppPublisher=CadProhlizec
DefaultDirName={autopf}\StepViewer
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; per-user install without admin rights; the user may choose "all users" in a dialog
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename=CadProhlizec_setup_{#AppVersion}{#Suffix}
SetupIconFile={#RepoDir}\assets\stepview.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
; smallest installer: LZMA2, 64 MB dictionary, one solid block
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "czech"; MessagesFile: "compiler:Languages\Czech.isl"

[Tasks]
Name: "desktopicon"; Description: "Vytvořit zástupce na ploše"; GroupDescription: "Další zástupci:"
Name: "thumbs"; Description: "Náhledy modelů a výkresů v Průzkumníku Windows (ikony souborů)"; GroupDescription: "Průzkumník:"
#ifndef NoDwg
Name: "thumbs\dwg"; Description: "i náhledy DWG (převodník LibreDWG – méně odolný vůči poškozeným / podvrženým souborům)"; GroupDescription: "Průzkumník:"; Flags: unchecked
#endif
Name: "assoc"; Description: "Otevírat 3D modely (.step .stp .iges .igs .stl) dvojklikem"; GroupDescription: "Asociace souborů:"
Name: "assoc2d"; Description: "Otevírat výkresy (.dxf .dwg) dvojklikem"; GroupDescription: "Asociace souborů:"; Flags: unchecked

[InstallDelete]
; old 1.0 shortcuts had a different name
Type: files; Name: "{autoprograms}\STEP prohlížeč.lnk"
Type: files; Name: "{autodesktop}\STEP prohlížeč.lnk"
; upgrade from 1.x: the PyInstaller folder with Python, Qt, VTK and the OCP DLLs, and the licence files of those libraries
Type: filesandordirs; Name: "{app}\_internal"
; the licence folder goes only while the DWG converter is not installed: the notice of the DWG add-on
; (licenses\THIRD_PARTY_NOTICES_DWG.txt) belongs to the converter and stays with it on a reinstallation
Type: files; Name: "{app}\licenses\*"; Check: not DwgConverterPresent

[Files]
#ifdef NoDwg
; the base build: the two notice files are written below, according to whether the DWG converter is installed
Source: "{#StageDir}\*"; Excludes: "StepThumb.dll,THIRD_PARTY_NOTICES.txt,THIRD_PARTY_NOTICES_DWG.txt"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; without the converter: the base notice (the converter is not in this installation)
Source: "{#StageDir}\licenses\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion; Check: not DwgConverterPresent
; with the converter (full installation or the add-on): its complete notice, and the base text is not written over the full one
Source: "{#StageDir}\licenses\THIRD_PARTY_NOTICES_DWG.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion; Check: DwgConverterPresent
#else
Source: "{#StageDir}\*"; Excludes: "StepThumb.dll"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
#endif
; the thumbnail DLL may be loaded by Explorer's thumbnail process -> replace on reboot if locked
Source: "{#StageDir}\StepThumb.dll"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; ---------- ProgIds
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueName: ""; ValueData: "3D model (STEP / IGES / STL)"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\{#ProgId2D}"; ValueType: string; ValueName: ""; ValueData: "Výkres (DXF / DWG)"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgId2D}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\{#ProgId2D}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
; ---------- always offer the app in "Open with…"
Root: HKA; Subkey: "Software\Classes\.step\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.stp\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.iges\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.igs\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.stl\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dxf\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId2D}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dwg\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId2D}"; ValueData: ""; Flags: uninsdeletevalue
; ---------- default handler
Root: HKA; Subkey: "Software\Classes\.step"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.stp"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.iges"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.igs"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.stl"; ValueType: string; ValueName: ""; ValueData: "{#ProgId}"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.dxf"; ValueType: string; ValueName: ""; ValueData: "{#ProgId2D}"; Flags: uninsdeletevalue; Tasks: assoc2d
Root: HKA; Subkey: "Software\Classes\.dwg"; ValueType: string; ValueName: ""; ValueData: "{#ProgId2D}"; Flags: uninsdeletevalue; Tasks: assoc2d
; ---------- Explorer thumbnail provider (.NET Framework 4 COM server, runs in the isolated thumbnail process)
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}"; ValueType: string; ValueName: ""; ValueData: "{#AppName} – náhledy"; Flags: uninsdeletekey; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: ""; ValueData: "mscoree.dll"; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: "ThreadingModel"; ValueData: "Both"; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: "Class"; ValueData: "StepThumb.StepThumbnailProvider"; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: "Assembly"; ValueData: "StepThumb, Version=1.3.0.0, Culture=neutral, PublicKeyToken=null"; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: "RuntimeVersion"; ValueData: "v4.0.30319"; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: "CodeBase"; ValueData: "{code:ThumbCodeBase}"; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.step\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.stp\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.iges\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.igs\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.stl\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dxf\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs
#ifndef NoDwg
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dwg\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs\dwg
#endif
; thumbnails unticked on (re)install -> remove the registration
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.step\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.stp\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.iges\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.igs\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.stl\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dxf\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
#ifndef NoDwg
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dwg\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs\dwg
#endif

[Run]
Filename: "{app}\{#AppExe}"; Description: "Spustit {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; the DWG converter folder, also when it was added later from the add-on zip, and the add-on's notices and readme
Type: filesandordirs; Name: "{app}\third_party"
Type: files; Name: "{app}\licenses\THIRD_PARTY_NOTICES_DWG.txt"
Type: files; Name: "{app}\PRECTI_ME_DWG.txt"
Type: filesandordirs; Name: "{localappdata}\StepViewer\cache"
; the settings in %APPDATA%\StepViewer are NOT listed here: the portable version keeps its settings there too, so they
; are removed only when the user asks (CurUninstallStepChanged), as 1.9 did (it removed only its cache)
; working directory of the sandboxed thumbnail renderer (Low integrity: %USERPROFILE%\AppData\LocalLow\StepViewerThumbs)
Type: filesandordirs; Name: "{localappdata}Low\StepViewerThumbs"

[Code]
const
  ThumbClsidStr = '{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}';
  ThumbIidStr = '{e357fccd-a995-4576-b01f-234630154e96}';

var
  { set on the ready page: a no-DWG installation over 1.9, whose converter sat in _internal (removed by [InstallDelete]) }
  UpgradeFrom19NoDwg: Boolean;

function ThumbCodeBase(Param: String): String;
var
  S: String;
begin
  S := ExpandConstant('{app}\StepThumb.dll');
  StringChangeEx(S, '\', '/', True);
  Result := 'file:///' + S;
end;

{ the thumbnail registration of this program for one extension, in the given root key; removed only when it points at
  our thumbnail handler (another program may register the same interface for the same extension) }
procedure RemoveThumbAssocIn(Root: Integer; Ext: String);
var
  K, V: String;
begin
  K := 'Software\Classes\SystemFileAssociations\' + Ext + '\ShellEx\' + ThumbIidStr;
  if RegQueryStringValue(Root, K, '', V) and (CompareText(V, ThumbClsidStr) = 0) then
    RegDeleteKeyIncludingSubkeys(Root, K);
end;

{ the per-user thumbnail registration that "Náhledy v Průzkumníku…" in the program writes to HKCU
  - also after a per-machine installation, where the [Registry] entries above live in HKLM and don't cover it }
procedure RemoveUserThumbAssoc(Ext: String);
begin
  RemoveThumbAssocIn(HKCU, Ext);
end;

{ the DWG converter (add-on) is installed in this installation }
function DwgConverterPresent: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\third_party\libredwg\dwg2dxf.exe'));
end;

{ the cadviewer:// link handler the program registers in HKCU on request (--register-link). Its command is
  "<exe>" --open-link "%1"; it is removed only when that exe is the installed one. A portable copy registers its own
  exe under the same key, and that registration stays with the portable copy. }
procedure RemoveLinkScheme;
var
  V, Exe: String;
begin
  Exe := ExpandConstant('{app}\{#AppExe}');
  if RegQueryStringValue(HKCU, 'Software\Classes\cadviewer\shell\open\command', '', V) then
    if Pos(LowerCase('"' + Exe + '" --open-link'), LowerCase(V)) = 1 then
      RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\cadviewer');
end;

{ 1.9 kept its DWG converter in _internal\libredwg; 2.0 has no such folder }
function ConverterIn19Folder: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\_internal\libredwg\dwg2dxf.exe'));
end;

procedure CurPageChanged(CurPageID: Integer);
begin
#ifdef NoDwg
  { read before the files are installed: [InstallDelete] removes _internal afterwards }
  if CurPageID = wpReady then
    UpgradeFrom19NoDwg := ConverterIn19Folder and (not DwgConverterPresent);
  if CurPageID = wpFinished then
    if UpgradeFrom19NoDwg then
      WizardForm.FinishedLabel.Caption := WizardForm.FinishedLabel.Caption + #13#10#13#10 +
        'Výkresy DWG: tato verze převodník DWG neobsahuje. Verze 1.9 ho měla ve složce _internal, která se při ' +
        'aktualizaci odstranila, takže soubory DWG se bez doplňku neotevřou. Doplněk StepViewer-dwg-addon-{#AppVersion}.zip ' +
        'rozbalte do složky programu (vznikne složka third_party\libredwg). DXF, STEP, IGES a STL fungují i bez něj.';
#endif
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
#ifdef NoDwg
    { the base build has no DWG option. With the converter installed, the DWG thumbnail registration stays while the
      thumbnails task is selected (as the full build's "thumbs\dwg" task); otherwise a registration left by an earlier
      installation is removed }
    if (not DwgConverterPresent) or (not WizardIsTaskSelected('thumbs')) then
    begin
      RemoveThumbAssocIn(HKCU, '.dwg');
      RemoveThumbAssocIn(HKLM, '.dwg');
    end;
#endif
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  SettingsDir: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    { the settings of 2.0 (%APPDATA%\StepViewer) are shared with the portable version: removed only when the user asks
      (default: keep); a silent uninstallation keeps them }
    SettingsDir := ExpandConstant('{userappdata}\StepViewer');
    if DirExists(SettingsDir) and (not UninstallSilent) then
      if MsgBox('Smazat také nastavení programu ve složce ' + SettingsDir + '?' + #13#10 + #13#10 +
                'Tuto složku může používat i přenosná verze programu. Pokud ji chcete ponechat, zvolte Ne.',
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(SettingsDir, True, True, True);
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    { program settings of 1.x (QSettings("CadProhlizec", "StepViewer"): window geometry, recent files, options) }
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\CadProhlizec\StepViewer');
    RegDeleteKeyIfEmpty(HKCU, 'Software\CadProhlizec');
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\CLSID\' + ThumbClsidStr);
    RemoveUserThumbAssoc('.step');
    RemoveUserThumbAssoc('.stp');
    RemoveUserThumbAssoc('.iges');
    RemoveUserThumbAssoc('.igs');
    RemoveUserThumbAssoc('.stl');
    RemoveUserThumbAssoc('.dxf');
    RemoveUserThumbAssoc('.dwg');
    RemoveLinkScheme;
  end;
end;
