import { ref } from "vue";
import "../shared/theme.js";

const mode = ref("dark");
const resolved = ref("dark");
const accent = ref("");
let wired = false;

function sync() {
  resolved.value =
    window.Theme?.resolved?.() ||
    (document.body.dataset.theme === "light" ? "light" : "dark");
  accent.value = document.body.style.getPropertyValue("--theme").trim();
}

function wire() {
  if (wired) return;
  wired = true;
  document.addEventListener("vexto:theme", sync);
}

export function useTheme() {
  async function init() {
    wire();
    try {
      await window.Theme?.apply?.();
    } catch {
      // 后端未就绪时保持 theme.css 的默认外观
    }
    try {
      const res = await window.__TAURI__?.core?.invoke("get_tool_config", {
        toolId: "general",
      });
      const v = res?.values || {};
      mode.value =
        v.appearance_theme === "light" || v.appearance_theme === "system"
          ? v.appearance_theme
          : "dark";
      accent.value = v.accent_color || "";
    } catch {
      mode.value = resolved.value;
    }
    sync();
  }

  async function toggle() {
    await window.Theme?.toggle?.();
    mode.value = resolved.value;
    sync();
  }

  return { mode, resolved, accent, init, toggle };
}
