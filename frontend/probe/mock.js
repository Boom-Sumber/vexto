// 探针页的 Tauri mock：只实现主题与窗口动作需要的最小接口。
// 必须在 App / 组件模块求值之前 import（entries/probe.js 的第一行）。
if (!window.__TAURI__) {
  const values = { appearance_theme: "dark", accent_color: "" };
  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        if (cmd === "get_tool_config") return { values };
        if (cmd === "save_tool_config") {
          Object.assign(values, args?.values || {});
          return {};
        }
        return {};
      },
    },
    event: {
      async listen() {
        return () => {};
      },
    },
    window: {
      getCurrentWindow() {
        return { startResizeDragging: async () => {} };
      },
    },
  };
}
