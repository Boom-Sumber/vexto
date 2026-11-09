// 自绘标题栏与无边框窗口的 8 向缩放。
//
// 页面只需要：
//   <header class="tb" data-tauri-drag-region>
//     <span class="tb-title" data-tauri-drag-region>标题</span>
//     <div class="tb-actions">
//       <button class="tb-btn" data-win="min"></button>
//       <button class="tb-btn" data-win="max"></button>
//       <button class="tb-btn close" data-win="close"></button>
//     </div>
//   </header>
// 然后 TitleBar.mount()。图标与按钮行为都由这里注入，页面不写 SVG 也不绑事件。
//
// 后端只有一个命令 `window_action(action)`，用当前窗口句柄执行：
// 比前端 window API 稳，也不依赖每个窗口单独授权。拖动用
// `data-tauri-drag-region`（需要 core:window:allow-start-dragging）。

window.TitleBar = (function () {
  const invoke = window.__TAURI__.core.invoke;
  const getCurrentWindow = window.__TAURI__.window.getCurrentWindow;

  const RESIZE = {
    n: "North",
    s: "South",
    e: "East",
    w: "West",
    ne: "NorthEast",
    nw: "NorthWest",
    se: "SouthEast",
    sw: "SouthWest",
  };

  function action(act) {
    invoke("window_action", { action: act }).catch((e) => {
      console.error("window_action", act, e);
    });
  }

  function mountIcons() {
    for (const b of document.querySelectorAll(".tb-btn[data-win]")) {
      const act = b.dataset.win;
      const name = act === "min" ? "minimize" : act === "max" ? "maximize" : "close";
      window.Icons.fill(b, name, 14);
      b.setAttribute(
        "aria-label",
        act === "min" ? "最小化" : act === "max" ? "最大化 / 还原" : "关闭"
      );
      b.addEventListener("click", () => action(act));
    }
  }

  function mountResize() {
    const host = document.querySelector(".win");
    if (!host) return;
    for (const cls of Object.keys(RESIZE)) {
      const el = document.createElement("div");
      el.className = "rz rz-" + cls;
      el.addEventListener("pointerdown", (ev) => {
        if (ev.button !== 0) return;
        ev.preventDefault();
        // 边缘拖拽：Tauri 原生处理，比用 JS 改窗口尺寸跟手
        getCurrentWindow()
          .startResizeDragging(RESIZE[cls])
          .catch(() => {});
      });
      host.appendChild(el);
    }
  }

  function mountFocus() {
    const sync = (on) => document.body.classList.toggle("win-blur", !on);
    window.addEventListener("focus", () => sync(true));
    window.addEventListener("blur", () => sync(false));
    sync(document.hasFocus());
  }

  function mount() {
    mountIcons();
    mountResize();
    mountFocus();
    const tb = document.querySelector(".tb");
    if (tb) {
      // 双击标题栏最大化 / 还原（Windows 习惯）
      tb.addEventListener("dblclick", (ev) => {
        if (ev.target.closest(".tb-actions")) return;
        action("max");
      });
    }
  }

  return { mount };
})();
