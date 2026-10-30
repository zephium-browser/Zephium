; Zephium registers itself as a browser per user when asked to become the
; default (desktop/src/default_browser/windows.rs). Uninstalling removes that
; registration so Default apps never lists a browser that is gone.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Clients\StartMenuInternet\Zephium"
  DeleteRegKey HKCU "Software\Classes\ZephiumURL"
  DeleteRegKey HKCU "Software\Classes\ZephiumHTML"
  DeleteRegValue HKCU "Software\RegisteredApplications" "Zephium"
!macroend
