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

  // 普通窗口是 `visible(false)` 建的：WebView2 首帧是白底，建出来就显示
  // 会先闪一下白屏。页面把自己的首屏数据填完后调 reveal()，后端才 show。
  // 划词弹窗不加载 titlebar.js，它走自己的 popup_ready。
  // **别用 rAF 等下一帧**：隐藏窗口里 Chromium 会暂停 rAF，窗口将永远不显示。
  let revealed = false;
  function reveal() {
    if (revealed) return;
    revealed = true;
    invoke("window_ready").catch(() => {});
  }

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
      // 页面可以挂一个 `window.__beforeClose`：返回 false 就表示「先别关」，
      // 典型场景是设置页有未保存的修改（要弹确认框）。没挂就是直接关。
      b.addEventListener("click", async () => {
        if (act === "close" && typeof window.__beforeClose === "function") {
          try {
            if ((await window.__beforeClose()) === false) return;
          } catch (e) {
            console.error("beforeClose", e);
          }
        }
        action(act);
      });
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

  /// `opts.resize === false`：固定尺寸窗口（如新建任务），不装 8 向缩放热区；
  /// `opts.maximize === false`：不可最大化，双击标题栏也不该触发。
  function mount(opts) {
    const o = opts || {};
    mountIcons();
    if (o.resize !== false) mountResize();
    mountFocus();

    // 兜底：页面脚本万一抛错（没调到 reveal），load 之后 1.5s 也必须显示窗口，
    // 否则用户会以为程序没反应。正常情况下页面 init 末尾会提前调 reveal()。
    const armFallback = () => setTimeout(reveal, 1500);
    if (document.readyState === "complete") armFallback();
    else window.addEventListener("load", armFallback, { once: true });
    const tb = document.querySelector(".tb");
    if (tb && o.maximize !== false) {
      // 双击标题栏最大化 / 还原（Windows 习惯）
      tb.addEventListener("dblclick", (ev) => {
        if (ev.target.closest(".tb-actions")) return;
        action("max");
      });
    }
  }

  return { mount, reveal };
})();
