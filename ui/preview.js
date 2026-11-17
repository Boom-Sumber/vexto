// 截图预览（会话 63）：图片走自定义 vextoimg 协议，裁剪由后端按像素行流式
// 完成。本页只负责显示与收集 head/tail，不接收图片字节（ADR-040 / ADR-044）。
//
// 会话 91 起增加标注：画布与图片重合，标注只在导出前合成一次 PNG，再按
// 原始字节提交给后端替换预览源（ADR-068）；关掉标注工具时画布点击穿透。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  const img = el("pv-img");
  const headEl = el("pv-head");
  const tailEl = el("pv-tail");
  const annot = el("pv-annot");
  const annotCtx = annot ? annot.getContext("2d") : null;
  const editBar = el("pv-edit-bar");
  const hintEl = el("pv-edit-hint");

  // theme.js 用 `const Theme` 定义，不挂 window；预览窗之前漏了这一步，始终暗色。
  if (typeof Theme !== "undefined") Theme.apply();

  let state = { path: "", width: 0, height: 0 };
  let head = 0;
  let tail = 0;
  let busy = false;
  let zoom = 1;

  // ── 标注状态 ─────────────────────────────────────────────────────────
  const MAX_EDIT_PIXELS = 16000000;
  let dpr = window.devicePixelRatio || 1;
  let tool = "";
  let color = "#f0566d";
  let lineWidth = 4;
  let ops = [];
  let history = [[]];
  let hp = 0;
  let draft = null;
  let dragId = null;
  let textInput = null;
  let renderQueued = false;
  let imgReady = false;
  const mosaicScratch = document.createElement("canvas");

  if (typeof Icons !== "undefined") {
    el("pv-tool-move").innerHTML = Icons.svg("pointer", 16);
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
      if (btn) btn.innerHTML = Icons.svg(icon, 16);
    }
    el("pv-undo").innerHTML = Icons.svg("undo", 16);
    el("pv-redo").innerHTML = Icons.svg("redo", 16);
    el("pv-clear").innerHTML = Icons.svg("trash", 16);
  }

  // 首屏状态拿到后再显示窗口（titlebar.js 的 reveal）；1.5s 兜底。

  function labels() {
    el("pv-head-v").textContent = head + " px";
    el("pv-tail-v").textContent = tail + " px";
  }

  function imageEditable() {
    const px = (state.width || 0) * (state.height || 0);
    return !!state.url && px > 0 && px <= MAX_EDIT_PIXELS;
  }

  function editable() {
    return !busy && imageEditable();
  }

  function applyState(p) {
    if (!p || !p.url) {
      el("pv-info").textContent = "没有可预览的图片";
      state = { path: "", width: 0, height: 0 };
      resetAnnotations();
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
    resetAnnotations();
    updateEditAvailability();
    // 标注导出需要读回画布像素；必须带 CORS 请求头，否则 canvas 会被标记为
    // tainted，toBlob 直接抛 SecurityError。vextoimg 已返回 ACAO: *。
    img.crossOrigin = "anonymous";
    imgReady = false;
    img.onload = () => {
      imgReady = true;
      applyZoom();
      updateBands();
      sizeAnnot();
      renderAnnot();
    };
    img.onerror = () => {
      imgReady = false;
    };
    img.src = state.url;
    applyZoom();
    sizeAnnot();
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

  // 预览图默认 1:1（按物理像素），不自动缩放；超出视口时由 .pv-viewport 滚动。
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
  }

  function msg(text) {
    el("pv-msg").textContent = text || "";
  }

  function setBusy(b) {
    busy = b;
    for (const id of ["pv-save", "pv-copy", "pv-pin", "pv-ocr", "pv-translate", "pv-copy-text"]) {
      el(id).disabled = b;
    }
    if (editBar) {
      for (const node of editBar.querySelectorAll("button, select")) node.disabled = b;
    }
    if (!b) updateEditAvailability();
  }

  // ── 标注：画布尺寸与可用性 ────────────────────────────────────────────
  function sizeAnnot() {
    if (!annot) return;
    if (!state.width || !state.height || !editable()) {
      annot.hidden = true;
      annot.width = 1;
      annot.height = 1;
      return;
    }
    annot.hidden = false;
    if (annot.width !== state.width || annot.height !== state.height) {
      annot.width = state.width;
      annot.height = state.height;
    }
    syncAnnotCss();
    renderAnnot();
  }

  function syncAnnotCss() {
    if (!annot || annot.hidden) return;
    annot.style.width = (img.clientWidth || 0) + "px";
    annot.style.height = (img.clientHeight || 0) + "px";
  }

  function updateEditAvailability() {
    const ok = editable();
    if (editBar) {
      for (const node of editBar.querySelectorAll("[data-tool], .pv-color, #pv-width")) {
        node.disabled = !ok;
      }
    }
    if (!ok) setTool("");
    updateUndoButtons();
    sizeAnnot();
    if (hintEl) {
      if (state.url && state.width && state.height && !editable() && !busy) {
        hintEl.textContent =
          "图片过大（" + state.width + "×" + state.height + "），暂不支持标注";
      } else {
        hintEl.textContent = tool
          ? "在图上拖动标注；Ctrl+Z 撤销"
          : "选择工具后在图上拖动标注";
      }
    }
  }

  function updateUndoButtons() {
    const ok = editable();
    el("pv-undo").disabled = !ok || hp <= 0;
    el("pv-redo").disabled = !ok || hp >= history.length - 1;
    el("pv-clear").disabled = !ok || ops.length === 0;
  }

  function resetAnnotations() {
    cancelTextInput();
    ops = [];
    history = [[]];
    hp = 0;
    draft = null;
    dragId = null;
    updateUndoButtons();
    renderAnnot();
  }

  function commitSnapshot() {
    history = history.slice(0, hp + 1);
    history.push(structuredClone(ops));
    hp = history.length - 1;
    updateUndoButtons();
  }

  function undoAnnot() {
    if (!editable() || hp <= 0) return;
    cancelTextInput();
    hp -= 1;
    ops = structuredClone(history[hp]);
    renderAnnot();
    updateUndoButtons();
  }

  function redoAnnot() {
    if (!editable() || hp >= history.length - 1) return;
    cancelTextInput();
    hp += 1;
    ops = structuredClone(history[hp]);
    renderAnnot();
    updateUndoButtons();
  }

  function clearAnnot() {
    if (!editable() || ops.length === 0) return;
    cancelTextInput();
    ops = [];
    commitSnapshot();
    renderAnnot();
  }

  function setTool(next) {
    // 切换 / 退出工具时丢弃未完成的笔画，避免 pointerup 丢失后留下残影。
    if (dragId !== null) {
      draft = null;
      dragId = null;
    }
    tool = editable() ? next || "" : "";
    if (editBar) {
      for (const btn of editBar.querySelectorAll(".pv-tool")) {
        btn.classList.toggle("on", (btn.dataset.tool || "") === tool);
      }
    }
    if (annot) {
      annot.classList.toggle("drawing", !!tool);
      annot.classList.toggle("text", tool === "text");
    }
    if (hintEl) {
      hintEl.textContent = tool
        ? "在图上拖动标注；Ctrl+Z 撤销"
        : "选择工具后在图上拖动标注";
    }
    if (!tool) cancelTextInput();
  }

  // ── 标注：绘制 ────────────────────────────────────────────────────────
  function renderAnnot() {
    if (!annotCtx || !annot || annot.hidden || !state.width || !state.height) return;
    const w = annot.width;
    const h = annot.height;
    annotCtx.clearRect(0, 0, w, h);
    if (!imgReady || !img.complete || !img.naturalWidth) return;
    annotCtx.drawImage(img, 0, 0, w, h);
    for (const op of ops) drawOp(annotCtx, op);
    if (draft) drawOp(annotCtx, draft);
  }

  function scheduleRender() {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      renderAnnot();
    });
  }

  function drawOp(ctx, op) {
    ctx.save();
    ctx.strokeStyle = op.color;
    ctx.fillStyle = op.color;
    ctx.lineWidth = Math.max(1, op.width || 2);
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    switch (op.tool) {
      case "rect":
        if (op.w > 0 && op.h > 0) ctx.strokeRect(op.x, op.y, op.w, op.h);
        break;
      case "ellipse":
        if (op.w > 0 && op.h > 0) {
          ctx.beginPath();
          ctx.ellipse(
            op.x + op.w / 2,
            op.y + op.h / 2,
            Math.max(op.w / 2, 0.5),
            Math.max(op.h / 2, 0.5),
            0,
            0,
            Math.PI * 2
          );
          ctx.stroke();
        }
        break;
      case "arrow":
        drawArrow(ctx, op);
        break;
      case "pen":
        drawPen(ctx, op, false);
        break;
      case "mosaic":
        drawMosaic(ctx, op);
        break;
      case "text":
        drawText(ctx, op);
        break;
      default:
        break;
    }
    ctx.restore();
  }

  function drawArrow(ctx, op) {
    const dx = op.x2 - op.x1;
    const dy = op.y2 - op.y1;
    const len = Math.hypot(dx, dy);
    if (len < 1) return;
    const ux = dx / len;
    const uy = dy / len;
    const head = Math.max(10, (op.width || 2) * 4);
    const bx = op.x2 - ux * head * 0.85;
    const by = op.y2 - uy * head * 0.85;
    ctx.beginPath();
    ctx.moveTo(op.x1, op.y1);
    ctx.lineTo(op.x2, op.y2);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(op.x2, op.y2);
    ctx.lineTo(bx + uy * head * 0.5, by - ux * head * 0.5);
    ctx.lineTo(bx - uy * head * 0.5, by + ux * head * 0.5);
    ctx.closePath();
    ctx.fill();
  }

  function drawPen(ctx, op, close) {
    if (!op.points.length) return;
    ctx.beginPath();
    ctx.moveTo(op.points[0].x, op.points[0].y);
    for (let i = 1; i < op.points.length; i++) {
      ctx.lineTo(op.points[i].x, op.points[i].y);
    }
    if (close) ctx.closePath();
    ctx.stroke();
  }

  function drawText(ctx, op) {
    if (!op.text) return;
    ctx.font =
      '600 ' + op.size + 'px "Microsoft YaHei", "Segoe UI", sans-serif';
    ctx.textBaseline = "top";
    ctx.fillText(op.text, op.x, op.y);
  }

  function drawMosaic(ctx, op) {
    if (!op.points.length || !img.naturalWidth) return;
    const block = Math.max(8, Math.round((op.width || 4) * 3 * dpr));
    const sw = state.width;
    const sh = state.height;
    let last = null;
    for (const p of op.points) {
      if (last && Math.hypot(p.x - last.x, p.y - last.y) < block / 2) continue;
      last = p;
      const sx = Math.max(0, Math.min(sw - block, Math.round(p.x - block / 2)));
      const sy = Math.max(0, Math.min(sh - block, Math.round(p.y - block / 2)));
      const bw = Math.min(block, sw - sx);
      const bh = Math.min(block, sh - sy);
      if (bw < 2 || bh < 2) continue;
      const tw = Math.max(1, Math.round(bw / 8));
      const th = Math.max(1, Math.round(bh / 8));
      mosaicScratch.width = tw;
      mosaicScratch.height = th;
      const tiny = mosaicScratch.getContext("2d");
      tiny.imageSmoothingEnabled = false;
      tiny.drawImage(img, sx, sy, bw, bh, 0, 0, tw, th);
      ctx.save();
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(mosaicScratch, 0, 0, tw, th, sx, sy, bw, bh);
      ctx.restore();
    }
  }

  // ── 标注：输入 ────────────────────────────────────────────────────────
  function toImage(ev) {
    const r = annot.getBoundingClientRect();
    const w = r.width || 1;
    const h = r.height || 1;
    return {
      x: Math.max(0, Math.min(annot.width, ((ev.clientX - r.left) / w) * annot.width)),
      y: Math.max(0, Math.min(annot.height, ((ev.clientY - r.top) / h) * annot.height)),
    };
  }

  function openText(p) {
    // 先提交上一个输入框，避免在画布上换位置时丢掉刚输入的文字。
    commitText();
    if (!annot || !annot.parentElement) return;
    const scale = annot.clientWidth ? annot.clientWidth / annot.width : 1;
    const size = Math.max(16 * dpr, lineWidth * 6 * dpr);
    const input = document.createElement("input");
    input.type = "text";
    input.className = "pv-text-input";
    input.maxLength = 500;
    input.style.left = p.x * scale + "px";
    input.style.top = p.y * scale + "px";
    input.style.fontSize = size * scale + "px";
    input.style.color = color;
    input.style.fontFamily = '"Microsoft YaHei", "Segoe UI", sans-serif';
    input.style.fontWeight = "600";
    input.style.lineHeight = "1.2";
    annot.parentElement.appendChild(input);
    textInput = { el: input, x: p.x, y: p.y, size };
    setTimeout(() => input.focus(), 0);
    input.addEventListener("keydown", (ev) => {
      if (ev.isComposing) return;
      if (ev.key === "Enter") {
        ev.preventDefault();
        commitText();
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        cancelTextInput();
      }
      ev.stopPropagation();
    });
    input.addEventListener("blur", () => commitText());
    input.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    input.addEventListener("mousedown", (ev) => ev.stopPropagation());
  }

  function commitText() {
    if (!textInput) return;
    const { el: input, x, y, size } = textInput;
    const text = input.value.trim();
    textInput = null;
    input.remove();
    if (!text) return;
    ops.push({ tool: "text", color, width: 0, x, y, text, size });
    commitSnapshot();
    renderAnnot();
  }

  function cancelTextInput() {
    if (!textInput) return;
    const { el: input } = textInput;
    textInput = null;
    input.remove();
  }

  // ── 标注：指针事件 ────────────────────────────────────────────────────
  if (annot) {
    annot.addEventListener("pointerdown", (ev) => {
      if (!editable() || !tool || ev.button !== 0) return;
      ev.preventDefault();
      const p = toImage(ev);
      if (tool === "text") {
        openText(p);
        return;
      }
      dragId = ev.pointerId;
      try {
        annot.setPointerCapture(ev.pointerId);
      } catch (_) {
        /* 指针捕获失败也能继续画 */
      }
      const base = { tool, color, width: lineWidth * dpr };
      if (tool === "pen" || tool === "mosaic") {
        draft = { ...base, points: [p] };
      } else if (tool === "arrow") {
        draft = { ...base, x1: p.x, y1: p.y, x2: p.x, y2: p.y };
      } else {
        draft = { ...base, x0: p.x, y0: p.y, x: p.x, y: p.y, w: 0, h: 0 };
      }
      scheduleRender();
    });

    annot.addEventListener("pointermove", (ev) => {
      if (dragId !== ev.pointerId || !draft) return;
      const p = toImage(ev);
      if (draft.tool === "pen" || draft.tool === "mosaic") {
        const last = draft.points[draft.points.length - 1];
        if (Math.hypot(p.x - last.x, p.y - last.y) >= 1.5) draft.points.push(p);
      } else if (draft.tool === "arrow") {
        draft.x2 = p.x;
        draft.y2 = p.y;
      } else {
        draft.x = Math.min(draft.x0, p.x);
        draft.y = Math.min(draft.y0, p.y);
        draft.w = Math.abs(p.x - draft.x0);
        draft.h = Math.abs(p.y - draft.y0);
      }
      scheduleRender();
    });

    const endDrag = (ev) => {
      if (dragId !== ev.pointerId) return;
      dragId = null;
      try {
        annot.releasePointerCapture(ev.pointerId);
      } catch (_) {
        /* 已释放 */
      }
      finishDraft();
    };
    annot.addEventListener("pointerup", endDrag);
    annot.addEventListener("pointercancel", endDrag);
  }

  function finishDraft() {
    const op = draft;
    draft = null;
    if (!op) return;
    if (op.tool === "pen" || op.tool === "mosaic") {
      if (op.points.length < 2) {
        renderAnnot();
        return;
      }
    } else if (op.tool === "arrow") {
      if (Math.hypot(op.x2 - op.x1, op.y2 - op.y1) < 3) {
        renderAnnot();
        return;
      }
    } else if (op.w < 2 || op.h < 2) {
      renderAnnot();
      return;
    }
    ops.push(op);
    commitSnapshot();
    renderAnnot();
  }

  // ── 交付：有标注时先合成一张 PNG 再走既有动作 ────────────────────────
  function composeEditedBlob() {
    return new Promise((resolve, reject) => {
      const w = state.width;
      const h = state.height;
      const outH = h - head - tail;
      if (!w || !h || outH <= 0) {
        reject(new Error("裁切范围不合法"));
        return;
      }
      const full = document.createElement("canvas");
      full.width = w;
      full.height = h;
      const ctx = full.getContext("2d");
      ctx.drawImage(img, 0, 0, w, h);
      for (const op of ops) drawOp(ctx, op);

      let out = full;
      if (head || tail) {
        out = document.createElement("canvas");
        out.width = w;
        out.height = outH;
        out.getContext("2d").drawImage(full, 0, head, w, outH, 0, 0, w, outH);
      }
      out.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error("合成编辑结果失败"));
      }, "image/png");
    });
  }

  async function act(kind) {
    if (busy) return;
    if (!state.url) {
      msg("没有可预览的图片");
      return;
    }
    setBusy(true);
    try {
      if (ops.length) {
        if (!imageEditable()) {
          msg("图片过大，暂不支持标注");
          return;
        }
        msg("正在应用标注…");
        let blob;
        try {
          blob = await composeEditedBlob();
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
    applyZoom();
    updateBands();
    syncAnnotCss();
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
      btn.addEventListener("click", () => setTool(btn.dataset.tool || ""));
    }
    const colors = el("pv-colors");
    if (colors) {
      colors.addEventListener("click", (ev) => {
        const btn = ev.target.closest(".pv-color");
        if (!btn || btn.disabled) return;
        color = btn.dataset.color || color;
        for (const node of colors.querySelectorAll(".pv-color")) {
          node.classList.toggle("on", node === btn);
        }
      });
    }
    el("pv-width").addEventListener("change", (ev) => {
      lineWidth = Math.max(1, Math.min(20, Number(ev.target.value) || 4));
    });
    el("pv-undo").addEventListener("click", undoAnnot);
    el("pv-redo").addEventListener("click", redoAnnot);
    el("pv-clear").addEventListener("click", clearAnnot);
  }

  el("pv-zoom-out").innerHTML = Icons.svg("minus", 16);
  el("pv-zoom-in").innerHTML = Icons.svg("plus", 16);
  el("pv-translate").innerHTML = Icons.raw("translate", 20);
  el("pv-copy-text").innerHTML = Icons.svg("copy", 20);

  el("pv-copy-text").addEventListener("click", () => {
    const text = el("pv-ocr-text").textContent;
    if (text) invoke("copy_text", { text }).catch(() => {});
  });

  const close = () => invoke("preview_close").catch(() => {});
  window.addEventListener("keydown", (e) => {
    if (textInput) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (e.shiftKey) redoAnnot();
      else undoAnnot();
      return;
    }
    if (mod && e.key.toLowerCase() === "y") {
      e.preventDefault();
      redoAnnot();
      return;
    }
    if (e.key === "Escape") {
      if (tool) {
        setTool("");
        return;
      }
      close();
    }
  });
})();
