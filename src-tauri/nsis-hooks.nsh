!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$SMPROGRAMS\${PRODUCTNAME} (Judge Demo).lnk" "$INSTDIR\${MAINBINARYNAME}.exe" "--judge-demo"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$SMPROGRAMS\${PRODUCTNAME} (Judge Demo).lnk"
!macroend
