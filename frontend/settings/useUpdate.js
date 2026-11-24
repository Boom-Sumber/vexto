import { computed, onBeforeUnmount, ref } from "vue";
import { t } from "../i18n/index.js";
import { errText } from "./utils.js";

// 应用更新状态：后端推整份快照（update:state），这里只按 phase 渲染。
// 关于页与侧栏提示共用同一份状态。
export function useUpdate(settings) {
  const invoke = window.__TAURI__.core.invoke;
  const listen = window.__TAURI__.event.listen;

  const state = ref(null);
  const info = ref(null);
  const busy = ref(false);
  // 侧栏「发现新版本」点击后要滚到关于页更新区；用自增令牌通知已挂载的页面。
  const focusToken = ref(0);

  let infoPromise = null;
  let unlisten = null;

  function loadInfo() {
    if (info.value) return Promise.resolve(info.value);
    if (!infoPromise) {
      infoPromise = invoke("about_info")
        .then((value) => {
          info.value = value;
          return value;
        })
        .catch((e) => {
          infoPromise = null;
          throw e;
        });
    }
    return infoPromise;
  }

  async function run(cmd, btn) {
    if (!cmd || busy.value) return;
    busy.value = true;
    if (btn) btn.disabled = true;
    try {
      const snapshot = await invoke(cmd);
      if (snapshot && snapshot.phase) state.value = snapshot;
    } catch (e) {
      settings.showToast(errText(e), "bad");
      try {
        state.value = await invoke("update_state");
      } catch {
        /* 拉不到就保持原样 */
      }
    } finally {
      busy.value = false;
      if (btn) btn.disabled = false;
    }
  }

  function requestInstall(cmd) {
    if (!cmd) return;
    // 安装会关掉当前进程：有未保存改动先让用户选保存 / 放弃。
    if (settings.pendingCount() > 0) {
      settings.showDirtyModal({ kind: "update", cmd });
      return;
    }
    run(cmd, null);
  }

  async function copyDiagnostics(btn) {
    if (btn) btn.disabled = true;
    try {
      const value = await loadInfo();
      const text = await invoke(value.diagnosticsCommand);
      await invoke("copy_text", { text });
      settings.showToast(t("settings.aboutCopied"));
    } catch (e) {
      settings.showToast(errText(e), "bad");
    } finally {
      if (btn) btn.disabled = false;
    }
  }

  async function openDir() {
    try {
      const value = await loadInfo();
      const res = await invoke(value.openDirCommand);
      if (res && res.ok === false) {
        settings.showToast(res.message || t("settings.aboutOpenDirFailed"), "bad");
      }
    } catch (e) {
      settings.showToast(errText(e), "bad");
    }
  }

  async function openLink(kind) {
    try {
      const value = await loadInfo();
      await invoke(value.openLinkCommand, { kind });
    } catch (e) {
      settings.showToast(errText(e), "bad");
    }
  }

  function jumpToUpdate() {
    focusToken.value += 1;
    if (settings.currentToolId() !== "about") {
      settings.selectTool("about", { keepBanner: true });
    }
  }

  async function badgeClick() {
    const s = state.value;
    if (s && s.phase === "ready") {
      try {
        const value = await loadInfo();
        const cmd = value.update && value.update.install;
        if (cmd) {
          requestInstall(cmd);
          return;
        }
      } catch {
        /* 读不到关于信息就退回普通跳转 */
      }
    }
    jumpToUpdate();
  }

  const badge = computed(() => {
    const s = state.value;
    const required = !!(s && s.required);
    const show = !!(s && s.latest) &&
      (required ? s.phase !== "up_to_date" : s.phase === "available");
    if (!show) return { show: false, required, text: "" };
    let text;
    if (required) {
      if (s.phase === "downloading") {
        const total = s.assetSize || 0;
        const got = s.downloaded || 0;
        text = total > 0
          ? t("settings.updateBadgeDownloading", {
              pct: Math.floor((got / total) * 100),
            })
          : t("settings.updateBadgeDownloadingUnknown");
      } else {
        text = t("settings.updateBadgeRequired", { version: s.latest });
      }
    } else {
      text = t("settings.updateBadgeAvailable", { version: s.latest });
    }
    return { show: true, required, text };
  });

  async function init() {
    try {
      unlisten = await listen("update:state", (ev) => {
        state.value = ev && ev.payload;
      });
    } catch {
      /* 事件注册失败不影响手动检查 */
    }
    try {
      state.value = await invoke("update_state");
    } catch {
      /* 首屏拉不到就等事件 */
    }
  }

  onBeforeUnmount(() => {
    if (typeof unlisten === "function") unlisten();
  });

  return {
    state,
    info,
    busy,
    focusToken,
    badge,
    init,
    loadInfo,
    run,
    requestInstall,
    copyDiagnostics,
    openDir,
    openLink,
    jumpToUpdate,
    badgeClick,
  };
}
