// 所有窗口禁用 WebView 默认右键菜单（2026-11-15 用户要求）。
// 捕获阶段拦截，覆盖页面内任何元素；文本输入框也一并禁用，键盘复制/粘贴不受影响。
window.addEventListener(
  "contextmenu",
  (ev) => {
    ev.preventDefault();
  },
  { capture: true }
);
