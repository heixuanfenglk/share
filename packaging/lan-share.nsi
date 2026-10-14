; 局域网文件分享 — NSIS 安装脚本
; 由仓库根目录的 pack.sh 调用 makensis 编译
; 在 macOS / Linux 上即可生成 Windows 安装包

Unicode true

!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef VERSION4
  !define VERSION4 "0.1.0.0"
!endif
!ifndef SOURCE_DIR
  !define SOURCE_DIR "..\target\release"
!endif
!ifndef OUT_DIR
  !define OUT_DIR "..\dist"
!endif
!ifndef ICON_FILE
  !define ICON_FILE "..\assets\icon.ico"
!endif
!ifndef SIZE_KB
  !define SIZE_KB 0
!endif
!ifndef README_FILE
  !define README_FILE "README.md"
!endif

Name "局域网文件分享"
OutFile "${OUT_DIR}/LanShare_Setup_${VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\局域网文件分享"
InstallDirRegKey HKCU "Software\LanShare" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails nevershow
BrandingText "局域网文件分享 ${VERSION}"

!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "欢迎安装局域网文件分享"
!define MUI_WELCOMEPAGE_TEXT "本向导将把局域网文件分享安装到当前用户目录，不需要管理员权限。$\r$\n$\r$\n安装后可把本机文件夹分享到同一局域网，其他设备用浏览器即可下载。$\r$\n$\r$\n点击“下一步”继续。"
!define MUI_FINISHPAGE_RUN "$INSTDIR\lan-share.exe"
!define MUI_FINISHPAGE_RUN_TEXT "启动局域网文件分享"
!define MUI_FINISHPAGE_TITLE "安装完成"
!define MUI_FINISHPAGE_TEXT "局域网文件分享已经安装完成。$\r$\n$\r$\n点击“完成”关闭向导。"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

VIProductVersion "${VERSION4}"
VIFileVersion "${VERSION4}"
VIAddVersionKey /LANG=2052 "ProductName" "局域网文件分享"
VIAddVersionKey /LANG=2052 "FileDescription" "局域网文件分享安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "lan-share"

LangString NAME_Main ${LANG_SIMPCHINESE} "主程序"
LangString NAME_Main ${LANG_ENGLISH} "Application"
LangString NAME_Desktop ${LANG_SIMPCHINESE} "桌面快捷方式"
LangString NAME_Desktop ${LANG_ENGLISH} "Desktop shortcut"
LangString DESC_Main ${LANG_SIMPCHINESE} "安装主程序（必选）"
LangString DESC_Main ${LANG_ENGLISH} "Install the application (required)"
LangString DESC_Desktop ${LANG_SIMPCHINESE} "在桌面创建快捷方式"
LangString DESC_Desktop ${LANG_ENGLISH} "Create a desktop shortcut"

Section "$(NAME_Main)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SOURCE_DIR}/lan-share.exe"
  File /nonfatal "${README_FILE}"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\局域网文件分享"
  CreateShortcut "$SMPROGRAMS\局域网文件分享\局域网文件分享.lnk" "$INSTDIR\lan-share.exe"
  CreateShortcut "$SMPROGRAMS\局域网文件分享\卸载局域网文件分享.lnk" "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "Software\LanShare" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "DisplayName" "局域网文件分享"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "Publisher" "lan-share"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "QuietUninstallString" "$\"$INSTDIR\Uninstall.exe$\" /S"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "DisplayIcon" "$INSTDIR\lan-share.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "NoRepair" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare" "EstimatedSize" ${SIZE_KB}
SectionEnd

Section /o "$(NAME_Desktop)" SecDesktop
  CreateShortcut "$DESKTOP\局域网文件分享.lnk" "$INSTDIR\lan-share.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "$(DESC_Main)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "$(DESC_Desktop)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\lan-share.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$SMPROGRAMS\局域网文件分享\局域网文件分享.lnk"
  Delete "$SMPROGRAMS\局域网文件分享\卸载局域网文件分享.lnk"
  RMDir "$SMPROGRAMS\局域网文件分享"
  Delete "$DESKTOP\局域网文件分享.lnk"
  ; 保留 lan-share.json。目录里还有用户配置时不会删掉安装目录。
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LanShare"
  DeleteRegKey HKCU "Software\LanShare"
SectionEnd
