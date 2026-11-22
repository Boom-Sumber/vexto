// 外观换肤：主题三态（dark / light / system）+ 主色调，五个窗口共用。
//
// 机制（外观功能会话，机制沿用会话 49 的 --theme 单源）：
//   - init 时读一次 `get_tool_config("general")`，把 appearance_theme 写成
//     body[data-theme]、accent_color 覆盖到 body 的 --theme 内联样式——
//     按钮 / 光晕 / LOGO 渐变全部经 CSS 相对色自动联动；
//   - 「跟随系统」用 matchMedia 解析并监听（WebView2 跟着 Windows 应用模式）；
//   - 设置页保存后后端会广播 `appearance:changed`，所有开着的窗口即时换肤；
//   - 设置页改动未保存时也要能预览：commit 会发 `vexto:committed`，
//     这里按 key 现算现应用（点「放弃」后重开窗口即回原值）。
//
// 本文件是这两个 key 的唯一消费者，允许出现 "appearance_theme" /
// "accent_color" 字面量——settings.js 的零分支红线（ADR-015）不适用这里。
//
// 用法：页面 <script src="theme.js"> 后在 init 里调 `Theme.apply()`。
// 窗口本就「先建隐藏、数据就绪才 show」，这里同步设变量，无首帧闪烁。

const Theme = (() => {
  const ACCENT_DEFAULT = ""; // 空 = 跟随主题 CSS 里的默认 #11a780

  let mql = null;

  function currentMode(theme) {
    return theme === "light" || theme === "system" ? theme : "dark";
  }

  /** 按值应用：theme ∈ dark/light/system，accent 为 #rrggbb 或空。 */
  function applyValues(theme, accent) {
    const mode = currentMode(theme);
    let resolved = mode;
    if (mode === "system") {
      resolved = window.matchMedia("(prefers-color-scheme: light)").matches
        ? "light"
        : "dark";
      watchSystem();
    } else {
      stopWatchSystem();
    }
    document.body.dataset.theme = resolved;
    if (accent && /^#[0-9a-fA-F]{6}$/.test(accent)) {
      document.body.style.setProperty("--theme", accent);
    } else {
      document.body.style.removeProperty("--theme");
    }
    document.dispatchEvent(new CustomEvent("vexto:theme", { detail: { theme: resolved } }));
  }

  /** system 模式下监听系统深浅切换；其它模式不挂监听器（随窗口销毁释放）。 */
  function watchSystem() {
    if (mql) return;
    mql = window.matchMedia("(prefers-color-scheme: light)");
    mql.addEventListener("change", onSystemChange);
  }
  function stopWatchSystem() {
    if (!mql) return;
    mql.removeEventListener("change", onSystemChange);
    mql = null;
  }
  let lastMode = "dark";
  function onSystemChange() {
    if (lastMode !== "system") {
      stopWatchSystem();
      return;
    }
    document.body.dataset.theme = window.matchMedia(
      "(prefers-color-scheme: light)",
    ).matches
      ? "light"
      : "dark";
    document.dispatchEvent(
      new CustomEvent("vexto:theme", { detail: { theme: document.body.dataset.theme } }),
    );
  }

  /** 页面 init：从后端读一次外观配置并应用。 */
  async function apply() {
    try {
      const res = await window.__TAURI__.core.invoke("get_tool_config", {
        toolId: "general",
      });
      const v = (res && res.values) || {};
      lastMode = currentMode(v.appearance_theme);
      applyValues(v.appearance_theme, v.accent_color || ACCENT_DEFAULT);
    } catch {
      // 后端没就绪 / 旧版本：保持主题 CSS 的默认样子（body 上已有 data-theme）
      lastMode = "dark";
    }
    listenBroadcast();
  }

  let listening = false;
  function listenBroadcast() {
    if (listening) return;
    listening = true;
    // 任何窗口保存外观 → 所有窗口即时换肤
    window.__TAURI__.event
      .listen("appearance:changed", (e) => {
        const p = e.payload || {};
        lastMode = currentMode(p.theme);
        applyValues(p.theme, p.accent || ACCENT_DEFAULT);
      })
      .catch(() => {
        listening = false;
      });
    // 设置页改动预览（未保存也跟着变）
    document.addEventListener("vexto:committed", (e) => {
      const d = e.detail || {};
      if (d.key === "appearance_theme") {
        lastMode = currentMode(d.value);
        applyValues(d.value, currentAccent());
      } else if (d.key === "accent_color") {
        applyValues(lastMode, d.value || ACCENT_DEFAULT);
      }
    });
  }

  function currentAccent() {
    return document.body.style.getPropertyValue("--theme").trim() || "";
  }

  /** 当前实际显示的主题（system 已解析成 light / dark）。 */
  function resolved() {
    return document.body.dataset.theme === "light" ? "light" : "dark";
  }

  /** 标题栏的明暗切换：立即应用，并写回配置（成功与否都不阻塞视觉切换）。 */
  async function toggle() {
    const next = resolved() === "light" ? "dark" : "light";
    lastMode = next;
    applyValues(next, currentAccent());
    try {
      await window.__TAURI__.core.invoke("save_tool_config", {
        toolId: "general",
        values: { appearance_theme: next },
      });
    } catch {
      // 配置写失败时界面已经切了，用户下次改设置会再落盘；不在这里弹错。
    }
    return next;
  }

  return { apply, applyValues, toggle, resolved };
})();

// titlebar.js 通过 window.Theme 访问；顶层 const 不会自动挂到 window 上。
window.Theme = Theme;
