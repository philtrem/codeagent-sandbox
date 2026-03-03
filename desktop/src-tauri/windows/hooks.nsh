; Kill sandbox.exe before install/uninstall so the installer can replace the binary.
; Uses the same nsis_tauri_utils plugin that Tauri uses for the main app binary.

!macro NSIS_HOOK_PREINSTALL
  !define SB_UID ${__LINE__}

  !if "${INSTALLMODE}" == "currentUser"
    nsis_tauri_utils::FindProcessCurrentUser "sandbox.exe"
  !else
    nsis_tauri_utils::FindProcess "sandbox.exe"
  !endif
  Pop $R0

  ${If} $R0 = 0
    ; sandbox.exe is running — try to kill it
    IfSilent sb_kill_${SB_UID} 0
    ${IfThen} $PassiveMode != 1 ${|} MessageBox MB_OKCANCEL "sandbox.exe is running!$\nClick OK to stop it so the installation can continue." IDOK sb_kill_${SB_UID} IDCANCEL sb_cancel_${SB_UID} ${|}

    sb_kill_${SB_UID}:
      !if "${INSTALLMODE}" == "currentUser"
        nsis_tauri_utils::KillProcessCurrentUser "sandbox.exe"
      !else
        nsis_tauri_utils::KillProcess "sandbox.exe"
      !endif
      Pop $R0
      Sleep 500

      ${If} $R0 = 0
      ${OrIf} $R0 = 2
        Goto sb_done_${SB_UID}
      ${Else}
        IfSilent sb_silent_fail_${SB_UID} 0
        Abort "Failed to stop sandbox.exe. Please close it manually then try again."
        sb_silent_fail_${SB_UID}:
          System::Call 'kernel32::AttachConsole(i -1)i.r0'
          ${If} $0 != 0
            System::Call 'kernel32::GetStdHandle(i -11)i.r0'
            System::Call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)'
            FileWrite $0 "sandbox.exe is running! Please close it first then try again.$\n"
          ${EndIf}
          Abort
      ${EndIf}

    sb_cancel_${SB_UID}:
      Abort "sandbox.exe is running! Please close it first then try again."
  ${EndIf}

  sb_done_${SB_UID}:
  !undef SB_UID
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !define SBU_UID ${__LINE__}

  !if "${INSTALLMODE}" == "currentUser"
    nsis_tauri_utils::FindProcessCurrentUser "sandbox.exe"
  !else
    nsis_tauri_utils::FindProcess "sandbox.exe"
  !endif
  Pop $R0

  ${If} $R0 = 0
    IfSilent sbu_kill_${SBU_UID} 0
    ${IfThen} $PassiveMode != 1 ${|} MessageBox MB_OKCANCEL "sandbox.exe is running!$\nClick OK to stop it so the uninstall can continue." IDOK sbu_kill_${SBU_UID} IDCANCEL sbu_cancel_${SBU_UID} ${|}

    sbu_kill_${SBU_UID}:
      !if "${INSTALLMODE}" == "currentUser"
        nsis_tauri_utils::KillProcessCurrentUser "sandbox.exe"
      !else
        nsis_tauri_utils::KillProcess "sandbox.exe"
      !endif
      Pop $R0
      Sleep 500

      ${If} $R0 = 0
      ${OrIf} $R0 = 2
        Goto sbu_done_${SBU_UID}
      ${Else}
        IfSilent sbu_silent_fail_${SBU_UID} 0
        Abort "Failed to stop sandbox.exe. Please close it manually then try again."
        sbu_silent_fail_${SBU_UID}:
          System::Call 'kernel32::AttachConsole(i -1)i.r0'
          ${If} $0 != 0
            System::Call 'kernel32::GetStdHandle(i -11)i.r0'
            System::Call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)'
            FileWrite $0 "sandbox.exe is running! Please close it first then try again.$\n"
          ${EndIf}
          Abort
      ${EndIf}

    sbu_cancel_${SBU_UID}:
      Abort "sandbox.exe is running! Please close it first then try again."
  ${EndIf}

  sbu_done_${SBU_UID}:
  !undef SBU_UID
!macroend
