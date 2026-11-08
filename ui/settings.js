// 设置页：APPID / 密钥 / 全局热键。
// 热键框是只读 + 捕获模式，不允许手打字符串（ADR-018）——
// 手打会写出 ctrl+t、Ctrl++T 这类解析不了或语义模糊的值。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const el = (id) => document.getElementById(id);

function setStatus(msg, bad) {
  const s = el("status");
  s.textContent = msg || "";
  s.className = bad ? "bad" : "";
}

// 热键注册失败的原因显示在该工具分区顶部（不是弹窗、不是控制台）
function showToolStatus(msg) {
  const box = el("tool-status");
  if (msg) {
    box.textContent = msg;
    box.classList.remove("hidden");
  } else {
    box.textContent = "";
    box.classList.add("hidden");
  }
}

// ── 热键捕获状态机（规格见 docs/stages/S2-配置与设置中心.md §2.6.3）────────
const MODS = ["Ctrl", "Shift", "Alt"];              // 白名单，顺序即归一化顺序
const MOD_KEYS = { Control: "Ctrl", Shift: "Shift", Alt: "Alt" };
const MAIN_RE = /^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-2]))$/;   // 主键白名单

const hotkeyEl = el("hotkey");
let capture = null;   // { el, prev, mods: [] }

function beginCapture(input) {
  if (capture) return;
  capture = { el: input, prev: input.value, mods: [] };
  input.value = "";
  input.placeholder = "请按下组合键…";
  input.classList.add("capturing");
  document.addEventListener("keydown", onKeyDown, true);
  input.addEventListener("blur", cancelCapture, { once: true });
}

function onKeyDown(e) {
  if (!capture) return;
  e.preventDefault();
  e.stopPropagation();

  if (e.key === "Escape") return cancelCapture();

  const mod = MOD_KEYS[e.key];
  if (mod) {
    // 第三个修饰键：静默取最后两个（Ctrl+Shift+Alt+T 是常见误操作，报错只会烦人）
    if (!capture.mods.includes(mod)) capture.mods.push(mod);
    if (capture.mods.length > 2) capture.mods = capture.mods.slice(-2);
    capture.el.value = normalizeMods(capture.mods) + "+";
    return;
  }

  // 主键：必须在白名单，且至少有一个修饰键
  if (!MAIN_RE.test(e.code)) return;          // 方向键、符号键、F13+ 一律忽略，不退出捕获
  if (capture.mods.length === 0) {
    capture.el.placeholder = "必须先按 Ctrl / Shift / Alt";
    return;
  }
  const main = e.code.startsWith("Key") ? e.code.slice(3)
             : e.code.startsWith("Digit") ? e.code.slice(5)
             : e.code;                        // F1..F12 原样
  capture.el.value = normalizeMods(capture.mods) + "+" + main;
  endCapture(true);
}

// item 顺序必须与 MODS 一致：Ctrl → Shift → Alt
function normalizeMods(mods) {
  return MODS.filter((m) => mods.includes(m)).join("+");
}

function endCapture(save) {
  if (!capture) return;
  const { el: input, prev } = capture;
  document.removeEventListener("keydown", onKeyDown, true);
  input.classList.remove("capturing");
  input.placeholder = "点击设置";
  if (!save) input.value = prev;
  capture = null;
  if (save) {
    // 即时保存：捕获完成即生效，不用再点「保存并生效」
    setStatus("热键已改为 " + input.value + "，正在保存…");
    saveAll();
  }
}

function cancelCapture() { endCapture(false); }

hotkeyEl.addEventListener("click", () => beginCapture(hotkeyEl));
hotkeyEl.addEventListener("focus", () => beginCapture(hotkeyEl));

// ── 读写配置 ──────────────────────────────────────────────────────────────
async function load() {
  try {
    const cfg = await invoke("get_config");
    el("appid").value = cfg.appid || "";
    el("secret").value = cfg.secret || "";
    hotkeyEl.value = cfg.hotkey || "Ctrl+Alt+T";
    if (cfg.config_path) el("cfgpath").textContent = cfg.config_path;
    showToolStatus(cfg.hotkey_error);
  } catch (e) {
    setStatus("读取配置失败：" + e, true);
  }
}

async function saveAll() {
  const payload = {
    // 源语言 / 目标语言界面上不露（S1b 能真翻译了再放），
    // 但必须原样带上，否则会被保存成空串覆盖掉配置里的默认值。
    from: "auto",
    to: "zh",
    appid: el("appid").value.trim(),
    secret: el("secret").value.trim(),
    hotkey: hotkeyEl.value.trim() || "Ctrl+Alt+T",
  };
  try {
    const res = await invoke("save_config", { config: payload });
    if (res && res.ok) {
      setStatus(res.message || "已保存");
      // 保存后重新拉一次：后端会顺手把归一化后的热键写回去
      const cfg = await invoke("get_config");
      hotkeyEl.value = cfg.hotkey || payload.hotkey;
      showToolStatus(cfg.hotkey_error);
    } else {
      setStatus((res && res.message) || "保存失败", true);
      if (res && res.message) showToolStatus(res.message);
    }
  } catch (e) {
    setStatus("保存失败：" + e, true);
  }
}

el("save").addEventListener("click", async () => {
  setStatus("保存中…");
  await saveAll();
});

window.addEventListener("DOMContentLoaded", load);
