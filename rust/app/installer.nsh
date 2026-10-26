; What the installer puts beside the program (tauri.conf.json: bundle > windows > nsis > installerHooks).
;
; The app finds its TASurf folder by plugin/scripting/tasurf.sp next to it, as in a source checkout, and keeps
; tasurf.local.json and out/ there, so it installs for the current user (%LOCALAPPDATA%\TASurf, writable). Beside
; it: the plugin (sources, the compiled plugin, the session config), the editor page, the example timeline and the
; licence. They aren't Tauri resources, which would also be copied next to the program in rust/target on every
; build and make that folder look like a TASurf folder.

!define TASURF_SRC "${__FILEDIR__}\..\.."  ; the repo, from rust/app/

!macro NSIS_HOOK_POSTINSTALL
  SetOutPath "$INSTDIR\plugin"
  File /r "${TASURF_SRC}\plugin\scripting"
  File /r "${TASURF_SRC}\plugin\cfg"
  SetOutPath "$INSTDIR\plugin\plugins"
  File "${TASURF_SRC}\plugin\plugins\tasurf.smx"
  SetOutPath "$INSTDIR\tasurf"
  File /r "${TASURF_SRC}\tasurf\editor_static"
  SetOutPath "$INSTDIR\timelines"
  File "${TASURF_SRC}\timelines\example_surf.json"
  SetOutPath "$INSTDIR"
  File "${TASURF_SRC}\LICENSE"
!macroend

; Settings (tasurf.local.json) stay unless "Delete the application data" is ticked, so an update keeps them. The
; project's timelines and videos are elsewhere (Documents\TASurf unless Setup picked another folder).
!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$INSTDIR\plugin"
  RMDir /r "$INSTDIR\tasurf"
  RMDir /r "$INSTDIR\out"
  Delete "$INSTDIR\timelines\example_surf.json"
  RMDir "$INSTDIR\timelines"
  Delete "$INSTDIR\LICENSE"
  ${If} $DeleteAppDataCheckboxState = 1
    Delete "$INSTDIR\tasurf.local.json"
  ${EndIf}
  RMDir "$INSTDIR"
!macroend
