// 所有窗口禁用 WebView 默认右键菜单（2026-11-15 用户要求）。
// 捕获阶段拦截，覆盖页面内任何元素；文本输入框也一并禁用，键盘复制/粘贴不受影响。
//
// Phase 2 起同时提供显式安装函数：Vue 引导只调一次，重复 import 不会重复注册。
let installed = false;

export function installContextMenu() {
  if (installed) return;
  installed = true;
  window.addEventListener(
    "contextmenu",
    (ev) => {
      ev.preventDefault();
    },
    { capture: true },
  );
}

installContextMenu();
