// 滚动截图进度窗（S7 会话 62）。
// 后端在工作线程跑主循环，这里只显示进度与三态结果；「停止」写一个
// 原子标志，主循环在下一拍看到就收尾。
import { t } from "../i18n/index.js";

export function startScrollControl(root) {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => root.querySelector("#" + id);
  let finished = false;
  let closeTimer = 0;
  let lastPath = "";

  if (window.Theme) Theme.apply();

  window.__TAURI__.event.listen("scroll:progress", (e) => {
    if (finished) return;
    const p = e.payload || {};
    const frames = p.frames || 0;
    const height = p.height || 0;
    el("scr-line").textContent =
      p.stage === "writing"
        ? t("scroll.writing")
        : t("scroll.progress", { frames, height });
    // 帧数上限是 100，用它当粗略进度；总高上限另看 height。
    const pct = Math.min(100, Math.max(4, Math.round((frames / 100) * 100)));
    el("scr-fill").style.width = pct + "%";
  });

  window.__TAURI__.event.listen("scroll:done", () => {
    if (finished) return;
    finished = true;
    // 结果与动作栏已改到选框浮层里显示；进度窗完成即关，避免两个 UI 打架。
    invoke("scroll_close").catch(() => {});
  });

  document.body.addEventListener("mouseenter", () => {
    if (closeTimer) {
      clearTimeout(closeTimer);
      closeTimer = 0;
    }
  });

  function openFromControl(cmd) {
    if (!lastPath) return;
    invoke(cmd, { path: lastPath })
      .then(() => invoke("scroll_close"))
      .catch(() => invoke("scroll_close").catch(() => {}));
  }
  el("scr-preview").addEventListener("click", () => openFromControl("preview_open"));
  el("scr-pin").addEventListener("click", () => openFromControl("pin_add"));

  el("scr-stop").addEventListener("click", () => {
    el("scr-stop").disabled = true;
    el("scr-line").textContent = t("scroll.stopping");
    invoke("scroll_stop").catch(() => {});
  });

  el("scr-close").addEventListener("click", () => {
    if (closeTimer) clearTimeout(closeTimer);
    invoke("scroll_close").catch(() => {});
  });

  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      if (closeTimer) clearTimeout(closeTimer);
      invoke("scroll_close").catch(() => {});
    }
  });
}
