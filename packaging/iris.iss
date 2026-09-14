; L'installateur d'Iris.
;
; Ce qu'il apporte, et qui ne peut pas être obtenu autrement — c'est la seule raison
; pour laquelle il existe :
;
;   1. un raccourci du menu Démarrer portant l'AppUserModelID « Iris.Mail ». Windows
;      n'affiche une notification que pour une identité qu'il connaît, et il ne connaît
;      que celles déclarées par un raccourci. Sans lui, aucune bulle n'apparaît ;
;   2. l'inscription mailto:, faite par l'application elle-même via « iris register » ;
;   3. une entrée « Ajouter ou supprimer des programmes », donc une désinstallation
;      qui retire ce qu'on a mis.
;
; Ce qu'il ne fait pas :
;
;   - il n'exige pas les droits d'administrateur. Tout va dans le profil de
;     l'utilisateur : ses réglages, son courrier, ses inscriptions. Demander une
;     élévation pour installer un client de courrier à une seule personne serait
;     demander plus que nécessaire, et imposer le logiciel aux autres comptes ;
;   - il n'efface pas les données à la désinstallation. Le courrier et les mots de
;     passe appartiennent à l'utilisateur, pas à l'installateur. Ce qui est retiré est
;     ce qui a été posé.
;
; Construction :  iscc packaging\iris.iss
; Sortie       :  packaging\output\iris-setup-<version>.exe

#define AppName        "Iris"
#define AppVersion     "0.1.0"
#define AppPublisher   "Iris"
#define AppExe         "iris.exe"
#define AppUserModelID "Iris.Mail"

[Setup]
AppId={{8C4E6D1A-3F52-4B7E-9A21-5D0E7C3F8B64}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}

; Dans le profil de l'utilisateur, sans élévation. C'est le choix central de ce
; fichier : « lowest » veut dire qu'aucune boîte de dialogue de contrôle de compte
; n'apparaît, et qu'aucun autre compte de la machine n'est touché.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto

OutputDir=output
OutputBaseFilename=iris-setup-{#AppVersion}
SetupIconFile=..\crates\iris-app\assets\iris.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

; Une application ouverte pendant la mise à jour verrouillerait son propre
; exécutable. Le dire vaut mieux que d'échouer à la copie.
CloseApplications=yes
CloseApplicationsFilter=*.exe
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked
Name: "mailto"; Description: "Offer Iris for mailto: links"; \
    Description/fr: "Proposer Iris pour les liens mailto:"
Name: "startup"; Description: "Start Iris when I sign in"; \
    Description/fr: "Démarrer Iris à l'ouverture de session"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md";                DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
; L'AppUserModelID sur le raccourci du menu Démarrer : c'est **la** ligne qui fait
; exister les notifications. Un raccourci sans elle est un raccourci ; avec elle,
; c'est une identité que Windows reconnaît.
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; \
    AppUserModelID: "{#AppUserModelID}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; \
    AppUserModelID: "{#AppUserModelID}"; Tasks: desktopicon

[Registry]
; Le démarrage à l'ouverture de session. « --tray » : se tenir prêt, pas ouvrir une
; fenêtre par-dessus ce que l'utilisateur fait dans les premières secondes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; \
    ValueType: string; ValueName: "{#AppName}"; \
    ValueData: """{app}\{#AppExe}"" --tray"; \
    Flags: uninsdeletevalue; Tasks: startup

[Run]
; L'inscription mailto: est faite par l'application, pas par l'installateur. Les deux
; écriraient les mêmes clés, et deux copies de la même vérité finissent par diverger :
; celle qui compte est celle que le code connaît.
Filename: "{app}\{#AppExe}"; Parameters: "register"; \
    StatusMsg: "Registering Iris as a mail client…"; \
    Flags: runhidden waituntilterminated; Tasks: mailto

Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; \
    Flags: nowait postinstall skipifsilent

[UninstallRun]
; Retirer avant de désinstaller : après, l'exécutable n'est plus là pour le faire, et
; les clés resteraient à pointer vers un fichier absent — Windows continuerait de
; proposer Iris comme client de courrier pour un programme qui n'existe plus.
Filename: "{app}\{#AppExe}"; Parameters: "unregister"; \
    Flags: runhidden waituntilterminated; RunOnceId: "UnregisterMailto"

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\Iris\Iris\cache"

; Le courrier, les réglages et le coffre ne sont **pas** effacés. Ils sont sous
; {userappdata}\Iris et appartiennent à l'utilisateur : une désinstallation qui
; emporte dix ans de courrier est une désinstallation qu'on ne pardonne pas. Le cache
; ci-dessus, lui, est entièrement reconstructible — c'est pour cela qu'il est séparé.
