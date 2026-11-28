// 截图预览（会话 63）：图片走自定义 vextoimg 协议，裁剪由后端按像素行流式
// 完成。本页只负责显示与收集 head/tail，不接收图片字节（ADR-040 / ADR-044）。
//
// 会话 95 起标注能力抽到 ui/annot_editor.js，与截图选区工具条共用。
import { t } from "../i18n/index.js";

export function startPreview(root, { onReady } = {}) {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => root.querySelector("#" + id);
  const img = el("pv-img");
  const saveMenu = el("pv-save-menu");
  const annot = el("pv-annot");
  const editBar = el("pv-edit-bar");
  const hintEl = el("pv-edit-hint");
  const cropBox = el("pv-crop-box");

  let state = { url: "", width: 0, height: 0 };
  let head = 0;
  let tail = 0;
  let left = 0;
  let right = 0;
  let busy = false;
  let zoom = 1;
  let dpr = window.devicePixelRatio || 1;
  let imgReady = false;
  let cropOn = false;
  let cropDrag = null;
  const MAX_EDIT_PIXELS = 16000000;
  const CROP_MIN = 8;
  const POP_TOOLS = new Set(["rect", "ellipse", "arrow", "pen", "mosaic", "text"]);

  const editor = window.AnnotEditor.create({
    canvas: annot,
    getScale: () => (annot.width ? (annot.clientWidth || 0) / annot.width : 1),
    onChange: () => {},
  });

  function imageEditable() {
    const px = (state.width || 0) * (state.height || 0);
    return !!state.url && px > 0 && px <= MAX_EDIT_PIXELS;
  }

  function editable() {
    return !busy && imageEditable() && imgReady;
  }

  function applyState(p) {
    if (!p || !p.url) {
      el("pv-info").textContent = t("preview.noImage");
      state = { url: "", width: 0, height: 0 };
      imgReady = false;
      setCropOn(false);
      editor.setEnabled(false);
      updateEditAvailability();
      return;
    }
    state = { url: p.url, width: p.width || 0, height: p.height || 0 };
    head = 0;
    tail = 0;
    left = 0;
    right = 0;
    zoom = 1;
    zoomLabel();
    el("pv-info").textContent = state.width + " × " + state.height + " px";
    msg("");
    el("pv-ocr-text").textContent = "";
    el("pv-trans-text").textContent = "";
    setCropOn(false);
    saveMenu.hidden = true;
    editor.setEnabled(false);
    img.crossOrigin = "anonymous";
    imgReady = false;
    img.onload = () => {
      imgReady = true;
      applyZoom();
      updateBands();
      paintCropBox();
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

  function cropScale() {
    return {
      sx: state.width ? (img.clientWidth || 0) / state.width : 0,
      sy: state.height ? (img.clientHeight || 0) / state.height : 0,
    };
  }

  /** 裁剪框直接画在图上：显示坐标 = 源像素 × 显示缩放。 */
  function paintCropBox() {
    if (!cropBox) return;
    const { sx, sy } = cropScale();
    if (!cropOn || !imgReady || !sx || !sy) {
      cropBox.hidden = true;
      return;
    }
    cropBox.hidden = false;
    cropBox.style.left = Math.round(left * sx) + "px";
    cropBox.style.top = Math.round(head * sy) + "px";
    cropBox.style.width = Math.max(1, Math.round((state.width - left - right) * sx)) + "px";
    cropBox.style.height = Math.max(1, Math.round((state.height - head - tail) * sy)) + "px";
  }

  /** 统一写入裁剪值，保证最小保留一块可见区域且不越出图片。 */
  function applyCrop(l, tp, rt, bt) {
    const w = state.width || 0;
    const h = state.height || 0;
    l = Math.max(0, Math.min(l, w - CROP_MIN));
    rt = Math.max(0, Math.min(rt, w - CROP_MIN - l));
    tp = Math.max(0, Math.min(tp, h - CROP_MIN));
    bt = Math.max(0, Math.min(bt, h - CROP_MIN - tp));
    left = Math.round(l);
    right = Math.round(rt);
    head = Math.round(tp);
    tail = Math.round(bt);
    updateBands();
    paintCropBox();
  }

  function setCropOn(next) {
    cropOn = !!next && editable();
    const btn = el("pv-crop");
    if (btn) btn.classList.toggle("on", cropOn);
    paintCropBox();
    updateEditAvailability();
  }

  function updateBands() {
    if (!state.height || !state.width || !img.clientHeight || !img.clientWidth) return;
    const sy = img.clientHeight / state.height;
    const sx = img.clientWidth / state.width;
    el("pv-band-top").style.height = Math.round(head * sy) + "px";
    el("pv-band-bottom").style.height = Math.round(tail * sy) + "px";
    el("pv-band-left").style.width = Math.round(left * sx) + "px";
    el("pv-band-right").style.width = Math.round(right * sx) + "px";
  }

  function applyZoom() {
    if (!state.width || !state.height) return;
    img.style.width = Math.round((state.width / dpr) * zoom) + "px";
    img.style.height = Math.round((state.height / dpr) * zoom) + "px";
    syncAnnotCss();
    setTimeout(() => {
      updateBands();
      paintCropBox();
    }, 0);
  }

  function setZoom(next) {
    zoom = Math.max(0.1, Math.min(4, next));
    applyZoom();
    zoomLabel();
    editor.render();
  }

  function zoomLabel() {
    const label = el("pv-zoom-label");
    if (label) label.textContent = Math.round(zoom * 100) + "%";
  }

  function syncAnnotCss() {
    if (!annot) return;
    annot.style.width = (img.clientWidth || 0) + "px";
    annot.style.height = (img.clientHeight || 0) + "px";
  }

  function setBusy(b) {
    busy = b;
    for (const id of [
      "pv-save",
      "pv-save-caret",
      "pv-copy",
      "pv-pin",
      "pv-ocr",
      "pv-translate",
      "pv-copy-text",
      "pv-crop",
    ]) {
      const node = el(id);
      if (node) node.disabled = b;
    }
    if (saveMenu) {
      for (const node of saveMenu.querySelectorAll("button")) node.disabled = b;
      if (b) saveMenu.hidden = true;
    }
    if (editBar) {
      for (const node of editBar.querySelectorAll("button")) node.disabled = b;
    }
    if (cropBox) cropBox.classList.toggle("busy", b);
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
    const crop = el("pv-crop");
    if (crop) crop.disabled = !ok;
    if (ok && !cropOn) {
      editor.setEnabled(true);
      editor.render();
    } else {
      editor.setEnabled(false);
    }
    if (hintEl) {
      if (state.url && state.width && state.height && !imageEditable()) {
        hintEl.textContent =
          t("preview.tooLargeNote", { w: state.width, h: state.height });
      } else if (cropOn) {
        hintEl.textContent = t("preview.cropHint");
      } else if (editor.tool) {
        hintEl.textContent = t("preview.editDraw");
      } else {
        hintEl.textContent = t("preview.editSelect");
      }
    }
  }

  function setTool(next) {
    const tool = editable() ? next || "" : "";
    if (cropOn) setCropOn(false);
    if (editBar) {
      for (const btn of editBar.querySelectorAll(".pv-tool")) {
        btn.classList.toggle("on", (btn.dataset.tool || "") === tool);
      }
    }
    editor.setTool(tool);
    if (!tool) editor.closePopup();
    if (hintEl) {
      hintEl.textContent = tool
        ? t("preview.editDraw")
        : t("preview.editSelect");
    }
  }

  let msgTimer = 0;
  function msg(text, kind) {
    const box = el("pv-msg");
    if (msgTimer) clearTimeout(msgTimer);
    msgTimer = 0;
    if (!box) return;
    box.textContent = text || "";
    box.className = "pv-msg" + (kind === "error" ? " bad" : "");
    if (!text || kind === "error" || kind === "progress") return;
    msgTimer = setTimeout(() => {
      box.textContent = "";
      box.className = "pv-msg";
      msgTimer = 0;
    }, 4000);
  }

  async function act(kind) {
    if (busy) return;
    if (!state.url) {
      msg(t("preview.noImage"), "error");
      return;
    }
    editor.commitPending();
    setBusy(true);
    try {
      if (editor.hasAnnotations()) {
        if (!imageEditable()) {
          msg(t("preview.tooLarge"), "error");
          return;
        }
        msg(t("preview.applyingAnnot"), "progress");
        let blob;
        try {
          blob = await editor.composeBlob({
            x: left,
            y: head,
            w: state.width - left - right,
            h: state.height - head - tail,
          });
        } catch (e) {
          msg(String(e), "error");
          return;
        }
        const r = await invoke("preview_commit_edited", await blob.arrayBuffer());
        if (!r || !r.ok) {
          msg((r && r.message) || t("preview.annotFailed"), "error");
          return;
        }
        applyState(r.state);
      }

      msg(kind === "ocr" ? t("preview.recognizing") : t("preview.processing"), "progress");
      const r = await invoke("preview_action", {
        head,
        tail,
        left,
        right,
        action: kind,
      });
      if (!r || !r.ok) {
        msg((r && r.message) || t("preview.actionFailed"), "error");
        return;
      }
      msg(r.message || t("preview.done"));
      if (kind === "ocr") {
        el("pv-ocr-text").textContent = r.text || "";
        el("pv-trans-text").textContent = "";
      }
    } catch (e) {
      msg(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function translateOcr() {
    const text = el("pv-ocr-text").textContent.trim();
    if (!text) {
      msg(t("preview.noText"), "error");
      return;
    }
    try {
      const r = await invoke("translate_text_only", { text });
      if (r && r.dst) {
        el("pv-trans-text").textContent = r.dst;
        msg("");
      } else {
        msg(t("preview.translateFailed"), "error");
      }
    } catch (e) {
      msg(String(e && e.message ? e.message : e), "error");
    }
  }

  invoke("preview_state")
    .then(applyState)
    .catch(() => {})
    .finally(() => onReady && onReady());
  window.__TAURI__.event.listen("preview:state", (e) => applyState(e.payload || {}));

  window.addEventListener("resize", () => {
    dpr = window.devicePixelRatio || 1;
    editor.setDpr(dpr);
    applyZoom();
    updateBands();
    paintCropBox();
    syncAnnotCss();
    editor.render();
    positionSaveMenu();
  });

  el("pv-save").addEventListener("click", () => act("save"));
  el("pv-copy").addEventListener("click", () => act("clipboard"));
  el("pv-pin").addEventListener("click", () => act("pin"));
  el("pv-ocr").addEventListener("click", () => act("ocr"));
  el("pv-translate").addEventListener("click", translateOcr);
  el("pv-zoom-out").addEventListener("click", () => setZoom(zoom / 1.25));
  el("pv-zoom-in").addEventListener("click", () => setZoom(zoom * 1.25));

  const cropBtn = el("pv-crop");
  const saveCaret = el("pv-save-caret");

  // 裁剪：按钮切换「裁剪框」模式，框直接盖在图上，8 个控制点可拖动。
  cropBtn.addEventListener("click", () => {
    const next = !cropOn;
    if (next) setTool("");
    setCropOn(next);
  });

  cropBox.addEventListener("pointerdown", (ev) => {
    if (!cropOn || ev.button !== 0) return;
    ev.preventDefault();
    ev.stopPropagation();
    cropBox.setPointerCapture(ev.pointerId);
    cropDrag = {
      id: ev.pointerId,
      handle: ev.target.dataset.crop || "",
      x: ev.clientX,
      y: ev.clientY,
      left,
      right,
      head,
      tail,
    };
  });

  cropBox.addEventListener("pointermove", (ev) => {
    if (!cropDrag || cropDrag.id !== ev.pointerId) return;
    const { sx, sy } = cropScale();
    if (!sx || !sy) return;
    const dx = (ev.clientX - cropDrag.x) / sx;
    const dy = (ev.clientY - cropDrag.y) / sy;
    const w = state.width;
    const h = state.height;
    let l = cropDrag.left;
    let r = cropDrag.right;
    let tp = cropDrag.head;
    let bt = cropDrag.tail;
    const key = cropDrag.handle;
    if (!key) {
      // 整体移动：尺寸不变，四边一起平移
      const boxW = w - l - r;
      const boxH = h - tp - bt;
      const nl = Math.max(0, Math.min(l + dx, w - boxW));
      const nt = Math.max(0, Math.min(tp + dy, h - boxH));
      l = nl;
      tp = nt;
      r = w - nl - boxW;
      bt = h - nt - boxH;
    } else {
      if (key.includes("w")) l = Math.min(Math.max(0, l + dx), w - r - CROP_MIN);
      if (key.includes("e")) r = Math.min(Math.max(0, r - dx), w - l - CROP_MIN);
      if (key.includes("n")) tp = Math.min(Math.max(0, tp + dy), h - bt - CROP_MIN);
      if (key.includes("s")) bt = Math.min(Math.max(0, bt - dy), h - tp - CROP_MIN);
    }
    applyCrop(l, tp, r, bt);
  });

  function endCropDrag(ev) {
    if (!cropDrag || cropDrag.id !== ev.pointerId) return;
    if (cropBox.hasPointerCapture?.(ev.pointerId)) {
      cropBox.releasePointerCapture(ev.pointerId);
    }
    cropDrag = null;
  }
  cropBox.addEventListener("pointerup", endCropDrag);
  cropBox.addEventListener("pointercancel", endCropDrag);

  function positionSaveMenu() {
    if (saveMenu.hidden) return;
    const r = saveCaret.getBoundingClientRect();
    const mw = saveMenu.offsetWidth || 128;
    const mh = saveMenu.offsetHeight || 72;
    let left = r.right - mw;
    left = Math.max(8, Math.min(window.innerWidth - mw - 8, left));
    let top = r.bottom + 6;
    if (top + mh > window.innerHeight - 8) top = Math.max(8, r.top - mh - 6);
    saveMenu.style.left = Math.round(left) + "px";
    saveMenu.style.top = Math.round(top) + "px";
  }

  function toggleSaveMenu(force) {
    const open = typeof force === "boolean" ? force : saveMenu.hidden;
    saveMenu.hidden = !open;
    saveCaret.setAttribute("aria-expanded", open ? "true" : "false");
    saveCaret.classList.toggle("on", open);
    if (open) positionSaveMenu();
  }

  saveCaret.addEventListener("click", () => toggleSaveMenu());
  for (const item of saveMenu.querySelectorAll("button[data-save]")) {
    item.addEventListener("click", () => {
      toggleSaveMenu(false);
      act(item.dataset.save);
    });
  }

  document.addEventListener(
    "pointerdown",
    (ev) => {
      if (!saveMenu.hidden && !saveMenu.contains(ev.target) && !saveCaret.contains(ev.target)) {
        toggleSaveMenu(false);
      }
    },
    true,
  );

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
    el("pv-crop").innerHTML = Icons.svg("crop", 16);
    el("pv-pin").innerHTML = Icons.svg("pin", 16);
    el("pv-copy").innerHTML = Icons.svg("copy", 16);
    el("pv-save-caret").innerHTML = Icons.svg("chevron-down", 14);
    el("pv-ocr-ico").innerHTML = Icons.svg("scan-text", 16);
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
      if (cropOn) {
        setCropOn(false);
        return;
      }
      if (editor.tool) {
        setTool("");
        return;
      }
      close();
    }
  });
}
