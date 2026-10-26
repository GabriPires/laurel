; Laurel's additions to the NSIS installer that Tauri's bundler writes (tools/installer.sh):
; Laurel and its engine closed before their files change, the Microsoft Visual C++
; Runtime when it is missing, start with Windows on a fresh install (spec app.startup),
; and on uninstall the game capture hook that win-capture shares with OBS Studio
; (wiki/projeto/runtime-obs.md). Tauri's template already removes the start entry
; (not on updates) and, when the user ticks its box, the app data. Clips are never
; removed.

; This file is UTF-8 with a BOM, the encoding makensis reads it in. The installer
; speaks English or Brazilian Portuguese (tauri.installer.conf.json); the messages
; here pick theirs by $LANGUAGE.
!define LAUREL_LANG_PT_BR 1046

; The start entry is named after the product, the value Tauri's uninstaller removes.
!define LAUREL_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define LAUREL_POWERSHELL "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
!define LAUREL_VC_KEY "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"
; The engine, OBS and ffmpeg.exe are built with the 14.44 toolset or older.
!define LAUREL_VC_MIN_MINOR 44
!define LAUREL_LAYERS_KEY "SOFTWARE\Khronos\Vulkan\ImplicitLayers"

; Closes Laurel, its engine and an ffmpeg run of this install, asking first when there
; is a window. Tauri's own check runs after the pre-hooks and knows only the app, and
; the engine leaves only after the app, so both are closed here.
!macro LAUREL_CLOSE_RUNNING
  nsExec::ExecToStack `"${LAUREL_POWERSHELL}" -NoProfile -NonInteractive -Command "@(Get-Process laurel-app -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' }).Count"`
  Pop $R0
  Pop $R1
  IntCmp $R1 0 laurel_close_done
  IfSilent laurel_close_now
  IntCmp $PassiveMode 1 laurel_close_now
  ${If} $LANGUAGE == ${LAUREL_LANG_PT_BR}
    MessageBox MB_OKCANCEL|MB_ICONINFORMATION "O Laurel está aberto. Fechar e continuar?" IDOK laurel_close_now
  ${Else}
    MessageBox MB_OKCANCEL|MB_ICONINFORMATION "Laurel is running. Close it and continue?" IDOK laurel_close_now
  ${EndIf}
  Abort
  laurel_close_now:
    nsExec::Exec `"${LAUREL_POWERSHELL}" -NoProfile -NonInteractive -Command "Get-Process laurel-app,laurel-engine,laurel-steam,ffmpeg -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force; Start-Sleep -Milliseconds 500"`
    Pop $R0
  laurel_close_done:
!macroend

; Installs the bundled Visual C++ Runtime unless 14.44 or newer is there. The runtime
; is machine-wide, so Windows asks for administrator rights only in that case.
!macro LAUREL_VC_RUNTIME
  SetRegView 64
  ReadRegDWORD $R0 HKLM "${LAUREL_VC_KEY}" "Installed"
  ReadRegDWORD $R1 HKLM "${LAUREL_VC_KEY}" "Major"
  ReadRegDWORD $R2 HKLM "${LAUREL_VC_KEY}" "Minor"
  SetRegView default
  ${If} $R0 == 1
  ${AndIf} $R1 == 14
  ${AndIf} $R2 >= ${LAUREL_VC_MIN_MINOR}
    DetailPrint "Microsoft Visual C++ Runtime $R1.$R2 found"
  ${Else}
    InitPluginsDir
    ; tools/installer.sh sets LAUREL_VC_REDIST to the checked download (Tauri copies
    ; this file into its build folder, so no path is relative to it).
    File "/oname=$PLUGINSDIR\vc_redist.x64.exe" "$%LAUREL_VC_REDIST%"
    DetailPrint "Installing the Microsoft Visual C++ Runtime"
    ExecWait '"$PLUGINSDIR\vc_redist.x64.exe" /install /quiet /norestart' $R3
    DetailPrint "Microsoft Visual C++ Runtime installer exit code: $R3"
    ; 3010: installed, restart pending; 1638: a newer version is installed.
    ${If} $R3 != 0
    ${AndIf} $R3 != 3010
    ${AndIf} $R3 != 1638
      ${If} $LANGUAGE == ${LAUREL_LANG_PT_BR}
        MessageBox MB_ICONEXCLAMATION|MB_OK "O Microsoft Visual C++ Runtime não foi instalado (código $R3). O Laurel precisa dele para gravar: instale o Visual C++ Redistributable (x64) da Microsoft ou rode este instalador de novo." /SD IDOK
      ${Else}
        MessageBox MB_ICONEXCLAMATION|MB_OK "The Microsoft Visual C++ Runtime was not installed (code $R3). Laurel needs it to record: install the Visual C++ Redistributable (x64) from Microsoft, or run this installer again." /SD IDOK
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

; Start with Windows is on after a fresh install, and the app confirms it on its first
; run. When Laurel already ran here (its settings exist), the user's choice stays:
; an existing entry is only pointed at this install.
!macro LAUREL_AUTOSTART
  IfFileExists "$LOCALAPPDATA\${BUNDLEID}\settings.json" laurel_ran laurel_fresh
  laurel_fresh:
    WriteRegStr HKCU "${LAUREL_RUN_KEY}" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --minimized'
    Goto laurel_autostart_done
  laurel_ran:
    ReadRegStr $R0 HKCU "${LAUREL_RUN_KEY}" "${PRODUCTNAME}"
    StrCmp $R0 "" laurel_autostart_done
    WriteRegStr HKCU "${LAUREL_RUN_KEY}" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --minimized'
  laurel_autostart_done:
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LAUREL_CLOSE_RUNNING
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro LAUREL_VC_RUNTIME
  !insertmacro LAUREL_AUTOSTART
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LAUREL_CLOSE_RUNNING
!macroend

; win-capture copies its hook to ProgramData and, without administrator rights,
; registers the Vulkan layer for this user. Both are shared with OBS Studio, so they
; stay when OBS is installed, and an update keeps them.
!macro NSIS_HOOK_POSTUNINSTALL
  IntCmp $UpdateMode 1 laurel_hook_done
  SetRegView 64
  ReadRegStr $R0 HKLM "SOFTWARE\OBS Studio" ""
  SetRegView 32
  ReadRegStr $R1 HKLM "SOFTWARE\OBS Studio" ""
  SetRegView default
  ${If} $R0 == ""
  ${AndIf} $R1 == ""
    ReadEnvStr $R2 PROGRAMDATA
    DeleteRegValue HKCU "${LAUREL_LAYERS_KEY}" "$R2\obs-studio-hook\obs-vulkan64.json"
    DeleteRegValue HKCU "${LAUREL_LAYERS_KEY}" "$R2\obs-studio-hook\obs-vulkan32.json"
    RMDir /r "$R2\obs-studio-hook"
    DetailPrint "Removed the game capture hook from $R2\obs-studio-hook"
  ${EndIf}
  laurel_hook_done:
!macroend
