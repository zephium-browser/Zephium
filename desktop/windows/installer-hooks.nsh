; Zephium registers itself as a browser per user when asked to become the
; default (desktop/src/default_browser/windows.rs), under names taken from its
; product name so Stable, Nightly and Dev each have their own. Uninstalling
; removes only this product's registration so Default apps never lists a
; browser that is gone.
!macro NSIS_HOOK_POSTUNINSTALL
  Push $R0
  Push $R1
  ; The URL class is the one this product's capabilities name, and only while
  ; it is still the class Zephium wrote for it.
  ReadRegStr $R0 HKCU "Software\Clients\StartMenuInternet\${PRODUCTNAME}\Capabilities\URLAssociations" "https"
  ${If} $R0 != ""
    ReadRegStr $R1 HKCU "Software\Classes\$R0" ""
    ${If} $R1 == "${PRODUCTNAME} Web Page"
      DeleteRegKey HKCU "Software\Classes\$R0"
    ${EndIf}
  ${EndIf}
  DeleteRegKey HKCU "Software\Clients\StartMenuInternet\${PRODUCTNAME}"
  DeleteRegValue HKCU "Software\RegisteredApplications" "${PRODUCTNAME}"
  Pop $R1
  Pop $R0
!macroend
