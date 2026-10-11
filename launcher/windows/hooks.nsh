; OSAgent install and uninstall hooks (Tauri NSIS template).
;
; The template's default check only stops the launcher exe. The core
; (osagent.exe), the updater and headless Chrome keep running and hold
; files and the server port, so they are stopped here first.

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping running OSAgent processes"
  nsExec::Exec 'taskkill /F /T /IM osagent-launcher.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM osagent.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM osagent-updater.exe'
  Pop $0
  ; Headless Chrome started by OSA runs from its own profile folder. Builds
  ; before the containment fix could leave it behind holding the server port.
  nsExec::Exec 'powershell.exe -NoProfile -NonInteractive -EncodedCommand RwBlAHQALQBDAGkAbQBJAG4AcwB0AGEAbgBjAGUAIABXAGkAbgAzADIAXwBQAHIAbwBjAGUAcwBzACAALQBGAGkAbAB0AGUAcgAgACIATgBhAG0AZQA9ACcAYwBoAHIAbwBtAGUALgBlAHgAZQAnACIAIAB8ACAAVwBoAGUAcgBlAC0ATwBiAGoAZQBjAHQAIAB7ACAAJABfAC4AQwBvAG0AbQBhAG4AZABMAGkAbgBlACAALQBsAGkAawBlACAAJwAqAFwALgBvAHMAYQBnAGUAbgB0AFwAYgByAG8AdwBzAGUAcgBcACoAJwAgAH0AIAB8ACAARgBvAHIARQBhAGMAaAAtAE8AYgBqAGUAYwB0ACAAewAgAFMAdABvAHAALQBQAHIAbwBjAGUAcwBzACAALQBJAGQAIAAkAF8ALgBQAHIAbwBjAGUAcwBzAEkAZAAgAC0ARgBvAHIAYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAGkAbABlAG4AdABsAHkAQwBvAG4AdABpAG4AdQBlACAAfQA='
  Pop $0
  Sleep 1000
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping running OSAgent processes"
  nsExec::Exec 'taskkill /F /T /IM osagent-launcher.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM osagent.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM osagent-updater.exe'
  Pop $0
  ; Headless Chrome started by OSA runs from its own profile folder. Builds
  ; before the containment fix could leave it behind holding the server port.
  nsExec::Exec 'powershell.exe -NoProfile -NonInteractive -EncodedCommand RwBlAHQALQBDAGkAbQBJAG4AcwB0AGEAbgBjAGUAIABXAGkAbgAzADIAXwBQAHIAbwBjAGUAcwBzACAALQBGAGkAbAB0AGUAcgAgACIATgBhAG0AZQA9ACcAYwBoAHIAbwBtAGUALgBlAHgAZQAnACIAIAB8ACAAVwBoAGUAcgBlAC0ATwBiAGoAZQBjAHQAIAB7ACAAJABfAC4AQwBvAG0AbQBhAG4AZABMAGkAbgBlACAALQBsAGkAawBlACAAJwAqAFwALgBvAHMAYQBnAGUAbgB0AFwAYgByAG8AdwBzAGUAcgBcACoAJwAgAH0AIAB8ACAARgBvAHIARQBhAGMAaAAtAE8AYgBqAGUAYwB0ACAAewAgAFMAdABvAHAALQBQAHIAbwBjAGUAcwBzACAALQBJAGQAIAAkAF8ALgBQAHIAbwBjAGUAcwBzAEkAZAAgAC0ARgBvAHIAYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAGkAbABlAG4AdABsAHkAQwBvAG4AdABpAG4AdQBlACAAfQA='
  Pop $0
  Sleep 1000
!macroend
