; fastdash installer (NSIS).
;
; Per-user, no elevation: installs to %LOCALAPPDATA%\fastdash and registers
; under HKCU, the same place and the same uninstall key the Tauri-built
; installer used, so it upgrades those installs in place.
;
; Built by the release workflow:
;   makensis /DVERSION=1.2.3 /DSOURCE=path\to\fastdash.exe /DOUTFILE=out.exe installer\fastdash.nsi
;
; Command-line switches, kept compatible with the Tauri updater that older
; installs run it from, and used by the in-app updater the same way:
;   /S       silent: no UI at all
;   /P       passive: progress only, no questions
;   /R       relaunch fastdash when done
;   /UPDATE  an update, not a fresh install: leave the desktop alone

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!ifndef VERSION
  !error "pass /DVERSION=X.Y.Z"
!endif
!ifndef SOURCE
  !error "pass /DSOURCE=path\to\fastdash.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "fastdash_${VERSION}_x64-setup.exe"
!endif

!define PRODUCT "fastdash"
!define PUBLISHER "zro"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}"
; makensis runs from this script's folder, so paths here are relative to it.
!define ICON "..\crates\fastdash-app\resources\icons\icon.ico"

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\${PRODUCT}"
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "FileDescription" "${PRODUCT} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "MIT"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING

Var Passive
Var Relaunch
Var Update

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!define MUI_FINISHPAGE_RUN "$INSTDIR\${PRODUCT}.exe"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/P" $1
  ${IfNot} ${Errors}
    StrCpy $Passive 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $0 "/R" $1
  ${IfNot} ${Errors}
    StrCpy $Relaunch 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $0 "/UPDATE" $1
  ${IfNot} ${Errors}
    StrCpy $Update 1
  ${EndIf}
  ; Passive runs close their own progress page when done.
  ${If} $Passive == 1
    SetAutoClose true
  ${EndIf}
FunctionEnd

Function SkipIfPassive
  ${If} $Passive == 1
    Abort
  ${EndIf}
FunctionEnd

; A running copy holds a lock on fastdash.exe, and its taskbar readout lives in
; the shell's window tree; ending the process cleans up both.
!macro StopRunning
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${PRODUCT}.exe'
  Pop $0
  Sleep 500
!macroend

Section "Install"
  !insertmacro StopRunning
  SetOutPath "$INSTDIR"
  File "/oname=${PRODUCT}.exe" "${SOURCE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; The Tauri build kept a WebView2 profile here. This app has no webview, so
  ; on an upgrade it is only dead weight.
  RMDir /r "$LOCALAPPDATA\co.zro.fastdash\EBWebView"
  RMDir "$LOCALAPPDATA\co.zro.fastdash"

  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${PRODUCT}.exe"
  ${If} $Update != 1
    CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${PRODUCT}.exe"
  ${EndIf}

  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" '"$INSTDIR\${PRODUCT}.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" '"$INSTDIR"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "MainBinaryName" "${PRODUCT}.exe"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Function .onInstSuccess
  ${If} $Relaunch == 1
    Exec '"$INSTDIR\${PRODUCT}.exe"'
  ${EndIf}
FunctionEnd

Section "Uninstall"
  !insertmacro StopRunning
  Delete "$INSTDIR\${PRODUCT}.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  ; Left by the Tauri-built installer.
  DeleteRegKey HKCU "Software\${PUBLISHER}\${PRODUCT}"
  ; Settings and tokens stay: the config lives under %APPDATA% and the tokens
  ; in the Windows Credential Manager, so a reinstall picks up where it left.
SectionEnd
