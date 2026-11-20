; Vexto 升级迁移：0.3.8 及更早版本的主程序叫 lightbox.exe。
; 0.3.9 起改为 Vexto.exe。升级前结束旧进程，安装前后各删一次旧文件；
; 旧进程如果还在运行，第一次删除会失败，POSTINSTALL 再补一次。
!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /IM lightbox.exe /F /T'
  Pop $0
  Sleep 500
  Delete "$INSTDIR\lightbox.exe"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Delete "$INSTDIR\lightbox.exe"
!macroend
