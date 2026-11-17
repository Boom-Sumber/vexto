// 截图预览（会话 63）：图片走自定义 vextoimg 协议，裁剪由后端按像素行流式
// 完成。本页只负责显示与收集 head/tail，不接收图片字节（ADR-040 / ADR-044）。
//
// 会话 95 起标注能力抽到 ui/annot_editor.js，与截图选区工具条共用。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  const img = el("pv-img");
  const headEl = el("pv-head");
  const tailEl = el("pv-tail");
  const annot = el("pv-annot");
  const editBar = el("pv-edit-bar");
  const hintEl = el("pv-edit-hint");

  if (typeof Theme !== "undefined") Theme.apply();

  let state = { url: "", width: 0, height: 0 };
  let head = 0;
  let tail = 0;
  let busy = false;
  let zoom = 1;
  let dpr = window.devicePixelRatio || 1;
  let imgReady = false;
  const MAX_EDIT_PIXELS = 16000000;
  const POP_TOOLS = new Set(["rect", "ellipse", "arrow", "pen", "mosaic", "text"]);

  const editor = window.AnnotEditor.create({
    canvas: annot,
    getScale: () => (annot.width ? (annot.clientWidth || 0) / annot.width : 1),
    onChange: () => {},
  });

  function labels() {
    el("pv-head-v").textContent = head + " px";
    el("pv-tail-v").textContent = tail + " px";
  }

  function imageEditable() {
    const px = (state.width || 0) * (state.height || 0);
    return !!state.url && px > 0 && px <= MAX_EDIT_PIXELS;
  }

  function editable() {
    return !busy && imageEditable() && imgReady;
  }

  function applyState(p) {
    if (!p || !p.url) {
      el("pv-info").textContent = "没有可预览的图片";
      state = { url: "", width: 0, height: 0 };
      imgReady = false;
      editor.setEnabled(false);
      updateEditAvailability();
      return;
    }
    state = { url: p.url, width: p.width || 0, height: p.height || 0 };
    head = 0;
    tail = 0;
    zoom = 1;
    headEl.max = Math.max(0, state.height - 1);
    tailEl.max = Math.max(0, state.height - 1);
    headEl.value = 0;
    tailEl.value = 0;
    labels();
    el("pv-info").textContent = state.width + " × " + state.height + " px";
    el("pv-msg").textContent = "";
    el("pv-ocr-box").hidden = true;
    el("pv-translation").hidden = true;
    el("pv-translation-text").textContent = "";
    editor.setEnabled(false);
    img.crossOrigin = "anonymous";
    imgReady = false;
    img.onload = () => {
      imgReady = true;
      applyZoom();
      updateBands();
      syncAnnotCss();
      editor.setDpr(dpr);
      editor.setSource(img, state.width, state.height);
      editor.setEnabled(editable());
      editor.render();
      updateEditAvailability();
    };
    img.onerror = () => {
      imgReady = false;
      editor.setEnabled(false);
    };
    img.src = state.url;
    applyZoom();
    syncAnnotCss();
    updateEditAvailability();
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

  function applyZoom() {
    if (!state.width || !state.height) return;
    img.style.width = Math.round((state.width / dpr) * zoom) + "px";
    img.style.height = Math.round((state.height / dpr) * zoom) + "px";
    syncAnnotCss();
    setTimeout(updateBands, 0);
  }

  function setZoom(next) {
    zoom = Math.max(0.1, Math.min(4, next));
    applyZoom();
    editor.render();
  }

  function syncAnnotCss() {
    if (!annot) return;
    annot.style.width = (img.clientWidth || 0) + "px";
    annot.style.height = (img.clientHeight || 0) + "px";
  }

  function setBusy(b) {
    busy = b;
    for (const id of ["pv-save", "pv-copy", "pv-pin", "pv-ocr", "pv-translate", "pv-copy-text"]) {
      el(id).disabled = b;
    }
    if (editBar) {
      for (const node of editBar.querySelectorAll("button")) node.disabled = b;
    }
    if (b) {
      editor.setEnabled(false);
    } else {
      updateEditAvailability();
    }
  }

  function updateEditAvailability() {
    const ok = editable();
    if (editBar) {
      for (const node of editBar.querySelectorAll("[data-tool]")) node.disabled = !ok;
    }
    if (ok) {
      editor.setEnabled(true);
      editor.render();
    } else {
      editor.setEnabled(false);
    }
    if (hintEl) {
      if (state.url && state.width && state.height && !imageEditable()) {
        hintEl.textContent =
          "图片过大（" + state.width + "×" + state.height + "），暂不支持标注";
      } else if (editor.tool) {
        hintEl.textContent = "拖动绘制；点已有图形可移动 / 缩放";
      } else {
        hintEl.textContent = "点图形可移动 / 缩放；选工具后拖动绘制";
      }
    }
  }

  function setTool(next) {
    const t = editable() ? next || "" : "";
    if (editBar) {
      for (const btn of editBar.querySelectorAll(".pv-tool")) {
        btn.classList.toggle("on", (btn.dataset.tool || "") === t);
      }
    }
    editor.setTool(t);
    if (!t) editor.closePopup();
    if (hintEl) {
      hintEl.textContent = t
        ? "拖动绘制；点已有图形可移动 / 缩放"
        : "点图形可移动 / 缩放；选工具后拖动绘制";
    }
  }

  function msg(text) {
    el("pv-msg").textContent = text || "";
  }

  async function act(kind) {
    if (busy) return;
    if (!state.url) {
      msg("没有可预览的图片");
      return;
    }
    editor.commitPending();
    setBusy(true);
    try {
      if (editor.hasAnnotations()) {
        if (!imageEditable()) {
          msg("图片过大，暂不支持标注");
          return;
        }
        msg("正在应用标注…");
        let blob;
        try {
          blob = await editor.composeBlob({
            x: 0,
            y: head,
            w: state.width,
            h: state.height - head - tail,
          });
        } catch (e) {
          msg(String(e));
          return;
        }
        const r = await invoke("preview_commit_edited", await blob.arrayBuffer());
        if (!r || !r.ok) {
          msg((r && r.message) || "应用标注失败");
          return;
        }
        applyState(r.state);
        head = 0;
        tail = 0;
        headEl.value = 0;
        tailEl.value = 0;
        labels();
      }

      msg(kind === "ocr" ? "识别中…" : "处理中…");
      const r = await invoke("preview_action", { head, tail, action: kind });
      if (!r || !r.ok) {
        msg((r && r.message) || "操作失败");
        return;
      }
      msg(r.message || "完成");
      if (kind === "ocr") {
        el("pv-ocr-box").hidden = false;
        el("pv-ocr-text").textContent = r.text || "";
        el("pv-translation").hidden = true;
        el("pv-translation-text").textContent = "";
      }
    } catch (e) {
      msg(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function translateOcr() {
    const text = el("pv-ocr-text").textContent;
    if (!text.trim()) {
      msg("没有可翻译的文字");
      return;
    }
    if (busy) return;
    setBusy(true);
    msg("翻译中…");
    try {
      const r = await invoke("translate_text_only", { text });
      if (!r || !r.dst) {
        msg("翻译失败");
        return;
      }
      el("pv-translation").hidden = false;
      el("pv-translation-text").textContent = r.dst;
      msg("已翻译");
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
  window.addEventListener("resize", () => {
    dpr = window.devicePixelRatio || 1;
    editor.setDpr(dpr);
    applyZoom();
    updateBands();
    syncAnnotCss();
    editor.render();
  });

  el("pv-save").addEventListener("click", () => act("save_as"));
  el("pv-copy").addEventListener("click", () => act("clipboard"));
  el("pv-pin").addEventListener("click", () => act("pin"));
  el("pv-ocr").addEventListener("click", () => act("ocr"));
  el("pv-translate").addEventListener("click", translateOcr);
  el("pv-zoom-out").addEventListener("click", () => setZoom(zoom / 1.25));
  el("pv-zoom-in").addEventListener("click", () => setZoom(zoom * 1.25));

  if (editBar) {
    for (const btn of editBar.querySelectorAll(".pv-tool")) {
      btn.addEventListener("click", () => {
        const next = btn.dataset.tool || "";
        setTool(next);
        if (next && POP_TOOLS.has(next)) editor.openPopup(btn);
        else editor.closePopup();
      });
    }
  }

  if (typeof Icons !== "undefined") {
    el("pv-tool-move").innerHTML = Icons.svg("pointer", 20);
    const ICONS = {
      rect: "rect",
      ellipse: "ellipse",
      arrow: "arrow-up-right",
      pen: "pen",
      text: "text",
      mosaic: "mosaic",
    };
    for (const [name, icon] of Object.entries(ICONS)) {
      const btn = editBar && editBar.querySelector('[data-tool="' + name + '"]');
      if (btn) btn.innerHTML = Icons.svg(icon, 20);
    }
    el("pv-zoom-out").innerHTML = Icons.svg("minus", 16);
    el("pv-zoom-in").innerHTML = Icons.svg("plus", 16);
    el("pv-translate").innerHTML = Icons.raw("translate", 20);
    el("pv-copy-text").innerHTML = Icons.svg("copy", 20);
  }

  el("pv-copy-text").addEventListener("click", () => {
    const text = el("pv-ocr-text").textContent;
    if (text) invoke("copy_text", { text }).catch(() => {});
  });

  const close = () => invoke("preview_close").catch(() => {});
  window.addEventListener("keydown", (e) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (e.shiftKey) editor.redo();
      else editor.undo();
      return;
    }
    if (mod && e.key.toLowerCase() === "y") {
      e.preventDefault();
      editor.redo();
      return;
    }
    if (e.key === "Escape") {
      if (editor.tool) {
        setTool("");
        return;
      }
      close();
    }
  });
})();
