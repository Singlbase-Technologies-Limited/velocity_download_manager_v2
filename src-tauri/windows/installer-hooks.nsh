; Hooks for the Windows installer Tauri generates (#81, docs/PACKAGING.md).
;
; Uninstalling removes V2's browser registrations: the keys below HKEY_CURRENT_USER that the app's
; Settings, Browser writes (crates/velocity-core/src/browser/registration.rs). Each is named after
; V2's release native-messaging host, so V1's (com.velocity.download_manager) is never touched. A
; test in src-tauri checks these names against branding.json.
;
; An upgrade runs the old uninstaller with /UPDATE ($UpdateMode = 1); the keys stay then, so the
; browsers keep working with the new version. The manifests they point to live in V2's data folder,
; which the uninstaller removes only when the user ticks "Delete the application data".

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.velocity.download_manager_v2"
    DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\com.velocity.download_manager_v2"
    DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.velocity.download_manager_v2"
    DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.velocity.download_manager_v2"
    DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.velocity.download_manager_v2"
  ${EndIf}
!macroend
