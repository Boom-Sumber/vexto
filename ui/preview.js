// 截图预览（会话 63）：图片走自定义 vextoimg 协议，裁剪由后端按像素行流式
// 完成。本页只负责显示与收集 head/tail，不接收图片字节（ADR-040 / ADR-044）。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  const img = el("pv-img");
  const headEl = el("pv-head");
  const tailEl = el("pv-tail");

  let state = { path: "", width: 0, height: 0 };
  let head = 0;
  let tail = 0;
  let busy = false;

  // 首屏状态拿到后再显示窗口（titlebar.js 的 reveal）；1.5s 兜底。

  function labels() {
    el("pv-head-v").textContent = head + " px";
    el("pv-tail-v").textContent = tail + " px";
  }

  function applyState(p) {
    if (!p || !p.url) {
      el("pv-info").textContent = "没有可预览的图片";
      return;
    }
    state = { url: p.url, width: p.width || 0, height: p.height || 0 };
    head = 0;
    tail = 0;
    headEl.max = Math.max(0, state.height - 1);
    tailEl.max = Math.max(0, state.height - 1);
    headEl.value = 0;
    tailEl.value = 0;
    labels();
    el("pv-info").textContent = state.width + " × " + state.height + " px";
    el("pv-msg").textContent = "";
    el("pv-ocr-box").hidden = true;
    img.onload = updateBands;
    img.src = state.url;
    updateBands();
  }

  function clamp() {
    const h = state.height || 1;
    head = Math.max(0, Math.min(Number(headEl.value) || 0, h - 1));
    tail = Math.max(0, Math.min(Number(tailEl.value) || 0, h - 1 - head));
    headEl.value = head;
    tailEl.value = tail;
    labels();
    updateBands();
  }

  function updateBands() {
    if (!state.height || !img.clientHeight) return;
    const s = img.clientHeight / state.height;
    el("pv-band-top").style.height = Math.round(head * s) + "px";
    el("pv-band-bottom").style.height = Math.round(tail * s) + "px";
  }

  function msg(text) {
    el("pv-msg").textContent = text || "";
  }

  function setBusy(b) {
    busy = b;
    for (const id of ["pv-save", "pv-copy", "pv-pin", "pv-ocr"]) {
      el(id).disabled = b;
    }
  }

  async function act(kind) {
    if (busy) return;
    if (!state.url) {
      msg("没有可预览的图片");
      return;
    }
    setBusy(true);
    msg(kind === "ocr" ? "识别中…" : "处理中…");
    try {
      const r = await invoke("preview_action", { head, tail, action: kind });
      if (!r || !r.ok) {
        msg((r && r.message) || "操作失败");
        return;
      }
      msg(r.message || "完成");
      if (kind === "ocr") {
        el("pv-ocr-box").hidden = false;
        el("pv-ocr-text").textContent = r.text || "";
      }
    } catch (e) {
      msg(String(e));
    } finally {
      setBusy(false);
    }
  }

  invoke("preview_state")
    .then(applyState)
    .catch(() => {})
    .finally(() => window.TitleBar?.reveal?.());
  window.__TAURI__.event.listen("preview:state", (e) => applyState(e.payload || {}));

  headEl.addEventListener("input", clamp);
  tailEl.addEventListener("input", clamp);
  window.addEventListener("resize", updateBands);

  el("pv-save").addEventListener("click", () => act("save"));
  el("pv-copy").addEventListener("click", () => act("clipboard"));
  el("pv-pin").addEventListener("click", () => act("pin"));
  el("pv-ocr").addEventListener("click", () => act("ocr"));

  el("pv-copy-text").addEventListener("click", () => {
    const text = el("pv-ocr-text").textContent;
    if (text) invoke("copy_text", { text }).catch(() => {});
  });

  const close = () => invoke("preview_close").catch(() => {});
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
  });
})();
