; 2.0.0 で表示名を「iPod Sync」から「ZenPod」に変えた。Windows のインストーラーはインストール先・
; アンインストールの登録を表示名で分けるので、そのままだと ZenPod が別のアプリとして入り、
; 古い「iPod Sync」がスタートメニューとアプリの一覧に残る。入れ終わったら古い版を静かに消す。
; /S ではアプリのデータを消すチェックが付かないので、設定・キャッシュ (識別子のフォルダ、ZenPod と共通) は残る。
!macro NSIS_HOOK_POSTINSTALL
  Push $R0
  Push $R1
  Push $R2
  Push $R3
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\iPod Sync" "InstallLocation"
  ; 値は "C:\...\iPod Sync" のように引用符付きで入っている
  StrCpy $R1 $R0 1
  ${If} $R1 == '"'
    StrCpy $R0 $R0 -1 1
  ${EndIf}
  ${If} $R0 != ""
  ${AndIf} ${FileExists} "$R0\uninstall.exe"
    ; 自動アップデート (/UPDATE) ではショートカットを作らない。古い版を消すとそちらのショートカットも
    ; 消えるので、あった場所にだけ ZenPod のものを作る
    StrCpy $R2 0
    StrCpy $R3 0
    ${If} ${FileExists} "$SMPROGRAMS\iPod Sync.lnk"
      StrCpy $R2 1
    ${EndIf}
    ${If} ${FileExists} "$DESKTOP\iPod Sync.lnk"
      StrCpy $R3 1
    ${EndIf}
    ; _?= を付けると一時フォルダに写さずその場で動くので、終わるまで待てる (代わりに自分は消せない)
    ExecWait '"$R0\uninstall.exe" /S _?=$R0'
    Delete "$R0\uninstall.exe"
    RMDir "$R0"
    ${If} $R2 = 1
    ${AndIfNot} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} $R3 = 1
    ${AndIfNot} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
      CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
!macroend
