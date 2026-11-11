; Inno Setup script – CAD prohlížeč (STEP / IGES / STL / DXF / DWG)
; Build: "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" installer\StepViewer.iss
; (or simply run build.py with --installer)

#define AppName "CAD prohlížeč"
#define AppVersion "1.9"
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
AppPublisher=CadProhlizec
DefaultDirName={autopf}\StepViewer
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; per-user install without admin rights; the user may choose "all users" in a dialog
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=CadProhlizec_setup_{#AppVersion}
SetupIconFile=..\stepview\stepview.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "czech"; MessagesFile: "compiler:Languages\Czech.isl"

[Tasks]
Name: "desktopicon"; Description: "Vytvořit zástupce na ploše"; GroupDescription: "Další zástupci:"
Name: "thumbs"; Description: "Náhledy modelů a výkresů v Průzkumníku Windows (ikony souborů)"; GroupDescription: "Průzkumník:"
Name: "thumbs\dwg"; Description: "i náhledy DWG (převodník LibreDWG – méně odolný vůči poškozeným / podvrženým souborům)"; GroupDescription: "Průzkumník:"; Flags: unchecked
Name: "assoc"; Description: "Otevírat 3D modely (.step .stp .iges .igs .stl) dvojklikem"; GroupDescription: "Asociace souborů:"
Name: "assoc2d"; Description: "Otevírat výkresy (.dxf .dwg) dvojklikem"; GroupDescription: "Asociace souborů:"; Flags: unchecked

[InstallDelete]
; old 1.0 shortcuts had a different name
Type: files; Name: "{autoprograms}\STEP prohlížeč.lnk"
Type: files; Name: "{autodesktop}\STEP prohlížeč.lnk"

[Files]
Source: "..\dist\StepViewer\*"; Excludes: "StepThumb.dll"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; the thumbnail DLL may be loaded by Explorer's thumbnail process -> replace on reboot if locked
Source: "..\dist\StepViewer\StepThumb.dll"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion

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
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dwg\ShellEx\{#ThumbIid}"; ValueType: string; ValueName: ""; ValueData: "{#ThumbClsid}"; Flags: uninsdeletekey; Tasks: thumbs\dwg
; thumbnails unticked on (re)install -> remove the registration
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.step\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.stp\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.iges\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.igs\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.stl\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dxf\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.dwg\ShellEx\{#ThumbIid}"; ValueType: none; Flags: deletekey; Tasks: not thumbs\dwg

[Run]
Filename: "{app}\{#AppExe}"; Description: "Spustit {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\StepViewer\cache"
; working directory of the sandboxed thumbnail renderer (Low integrity: %USERPROFILE%\AppData\LocalLow\StepViewerThumbs)
Type: filesandordirs; Name: "{localappdata}Low\StepViewerThumbs"

[Code]
const
  ThumbClsidStr = '{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}';
  ThumbIidStr = '{e357fccd-a995-4576-b01f-234630154e96}';

function ThumbCodeBase(Param: String): String;
var
  S: String;
begin
  S := ExpandConstant('{app}\StepThumb.dll');
  StringChangeEx(S, '\', '/', True);
  Result := 'file:///' + S;
end;

{ the per-user thumbnail registration that "Náhledy v Průzkumníku…" in the program (thumbsettings.py) writes to HKCU
  - also after a per-machine installation, where the [Registry] entries above live in HKLM and don't cover it }
procedure RemoveUserThumbAssoc(Ext: String);
var
  K, V: String;
begin
  K := 'Software\Classes\SystemFileAssociations\' + Ext + '\ShellEx\' + ThumbIidStr;
  if RegQueryStringValue(HKCU, K, '', V) and (CompareText(V, ThumbClsidStr) = 0) then
    RegDeleteKeyIncludingSubkeys(HKCU, K);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    { program settings (QSettings("CadProhlizec", "StepViewer"): window geometry, recent files, options) }
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
  end;
end;
