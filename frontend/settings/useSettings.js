import { computed, onBeforeUnmount, ref, watch } from "vue";
import { t } from "../i18n/index.js";
import { useToast } from "../composables/useToast.js";
import { errText, stableJson } from "./utils.js";

export function useSettings() {
  const invoke = window.__TAURI__.core.invoke;
  const listen = window.__TAURI__.event.listen;
  const toast = useToast();

  const tools = ref([]);
  const toolId = ref(null);
  const sections = ref([]);
  const tabId = ref(null);
  const values = ref({});
  const pending = ref({});
  const status = ref("");
  const statusKind = ref("");
  const banner = ref("");
  const loading = ref(true);
  const dirtyModal = ref(null);

  let statusTimer = 0;
  let bannerTimer = 0;
  const autoApplyTimers = {};
  let unlistenGoto = null;
  let unlistenAppearance = null;

  const tool = computed(() => tools.value.find((x) => x.id === toolId.value) || null);
  const section = computed(
    () => sections.value.find((s) => s.id === tabId.value) || sections.value[0] || null,
  );
  const fields = computed(() => section.value?.fields || []);
  const pendingCount = computed(() => Object.keys(pending.value).length);

  function val(key) {
    // 必须用 `in`，不能用 hasOwnProperty.call：后者不走 Proxy 的 has trap，
    // Vue 收不到「新增 pending 键」的依赖，开关 / show_if 不会重渲染。
    if (key in pending.value) return pending.value[key];
    return values.value[key];
  }

  function setStatus(msg, kind) {
    status.value = msg || "";
    statusKind.value = kind === true ? "bad" : typeof kind === "string" ? kind : "";
    clearTimeout(statusTimer);
    if (msg && kind !== "dirty") {
      statusTimer = setTimeout(() => {
        if (pendingCount.value > 0) return;
        status.value = "";
        statusKind.value = "";
      }, 4000);
    }
  }

  function showBanner(msg) {
    clearTimeout(bannerTimer);
    banner.value = msg || "";
    if (msg) {
      bannerTimer = setTimeout(() => {
        banner.value = "";
      }, 8000);
    }
  }

  function showToast(msg, kind) {
    if (!msg) return;
    toast.show(msg, { kind: kind === "bad" ? "error" : "info" });
  }

  function commit(key, value, opts = {}) {
    if (opts.autoApply) {
      if (stableJson(values.value[key]) !== stableJson(value)) {
        values.value[key] = value;
        // 取色器拖动会连续触发 input：250ms 内的改动合并成一次落盘；
        // 未保存预览仍由下面的 vexto:committed 每个事件即时生效。
        clearTimeout(autoApplyTimers[key]);
        autoApplyTimers[key] = setTimeout(async () => {
          delete autoApplyTimers[key];
          try {
            const res = await invoke("save_tool_config", {
              toolId: toolId.value,
              values: { [key]: value },
            });
            if (!res || !res.ok) {
              showBanner((res && res.message) || t("settings.saveFailed"));
            }
          } catch (e) {
            showBanner(t("settings.saveFailedWith", { message: errText(e) }));
          }
        }, 250);
      }
      document.dispatchEvent(new CustomEvent("vexto:committed", { detail: { key, value } }));
      return;
    }

    const original = values.value[key];
    if (stableJson(original) === stableJson(value)) {
      delete pending.value[key];
    } else {
      pending.value[key] = value;
    }
    // 外观字段（主题 / 主色调）改动要能未保存就预览。
    document.dispatchEvent(new CustomEvent("vexto:committed", { detail: { key, value } }));
  }

  async function reloadTools() {
    try {
      tools.value = await invoke("list_tools");
    } catch (e) {
      setStatus(t("settings.readToolsFailed", { message: errText(e) }), true);
    }
  }

  async function reloadValues() {
    try {
      const res = await invoke("get_tool_config", { toolId: toolId.value });
      values.value = (res && res.values) || {};
    } catch {
      /* 读不回来就保持现状，至少不崩 */
    }
  }

  async function refreshHotkeyError() {
    try {
      const msg = await invoke("hotkey_error");
      if (msg) showBanner(msg);
    } catch {
      /* 没有就不显示 */
    }
  }

  async function refreshAfterSave() {
    await reloadTools();
    if (tools.value.some((x) => x.id === toolId.value)) {
      await refreshHotkeyError();
    }
  }

  async function selectTool(id, opts = {}) {
    if (!id) return;
    // 有未保存改动又切到别的工具：先问。点同一个工具不算切换，pending 留着。
    if (!opts.skipGuard && pendingCount.value > 0 && id !== toolId.value) {
      showDirtyModal({ kind: "switch", toolId: id });
      return;
    }
    const found = tools.value.find((x) => x.id === id);
    if (!found) return;

    toolId.value = id;
    if (!opts.keepBanner) showBanner("");
    sections.value = found.sections || [];
    tabId.value =
      opts.tabId ||
      (sections.value.length === 1 ? sections.value[0].id : sections.value[0]?.id) ||
      null;

    try {
      const res = await invoke("get_tool_config", { toolId: id });
      values.value = (res && res.values) || {};
    } catch (e) {
      setStatus(t("settings.readConfigFailed", { message: errText(e) }), true);
      values.value = {};
    }
    await refreshHotkeyError();
  }

  async function toggleToolEnabled(want) {
    const found = tool.value;
    if (!found) return;
    try {
      const res = await invoke("set_tool_enabled", { toolId: found.id, enabled: want });
      if (res && res.ok) {
        setStatus(res.message || t("settings.saved"));
        showBanner("");
        await reloadTools();
        await selectTool(found.id, { keepBanner: true });
      } else {
        showBanner((res && res.message) || t("settings.toggleFailed"));
        setStatus("");
        return false;
      }
    } catch (e) {
      showBanner(t("settings.toggleFailedWith", { message: errText(e) }));
      return false;
    }
    return true;
  }

  async function saveAll() {
    if (pendingCount.value === 0) return true;
    setStatus(t("settings.saving"));
    try {
      const res = await invoke("save_tool_config", {
        toolId: toolId.value,
        values: pending.value,
      });
      if (!res || !res.ok) {
        setStatus((res && res.message) || t("settings.saveFailed"), true);
        showBanner((res && res.message) || t("settings.saveFailed"));
        return false;
      }
      pending.value = {};
      // 后端会改写部分值（清空目录回写默认路径），所以保存后重读一次。
      await reloadValues();
      setStatus(res.message || t("settings.saved"));
      if (Array.isArray(res.warnings) && res.warnings.length) {
        showBanner(res.warnings.join("\n"));
      } else {
        showBanner("");
      }
      await refreshAfterSave();
      return true;
    } catch (e) {
      setStatus(t("settings.saveFailedWith", { message: errText(e) }), true);
      showBanner(t("settings.saveFailedWith", { message: errText(e) }));
      return false;
    }
  }

  async function discardAll() {
    pending.value = {};
    await reloadValues();
    setStatus(t("settings.discardDone"));
  }

  function showDirtyModal(intent) {
    dirtyModal.value = intent;
  }

  function hideDirtyModal() {
    dirtyModal.value = null;
  }

  const modalText = computed(() => {
    const intent = dirtyModal.value;
    if (!intent) return "";
    const n = pendingCount.value;
    const name = tool.value?.name || t("settings.title");
    if (intent.kind === "close") return t("settings.dirtyClose", { name, n });
    if (intent.kind === "update") return t("settings.dirtyUpdate", { name, n });
    return t("settings.dirtySwitch", { name, n });
  });

  function runIntent(intent) {
    if (!intent) return;
    if (intent.kind === "close") {
      invoke("window_action", { action: "close" }).catch(() => {});
    } else if (intent.kind === "switch") {
      selectTool(intent.toolId, { skipGuard: true });
    }
  }

  async function resolveDirty(action) {
    const intent = dirtyModal.value;
    if (!intent) return;
    if (action === "cancel") {
      hideDirtyModal();
      return;
    }
    if (action === "discard") {
      hideDirtyModal();
      pending.value = {};
      setStatus(t("settings.discardDone"));
      runIntent(intent);
      return;
    }
    if (action === "save") {
      const ok = await saveAll();
      if (!ok) return; // 保存失败就停在弹窗里
      hideDirtyModal();
      runIntent(intent);
    }
  }

  async function init() {
    await reloadTools();
    let wanted = null;
    try {
      wanted = await invoke("take_pending_tool");
    } catch {
      /* 没有就算了 */
    }
    const first =
      wanted && tools.value.some((x) => x.id === wanted) ? wanted : tools.value[0]?.id;
    if (first) await selectTool(first);
    loading.value = false;

    window.__beforeClose = async () => {
      if (pendingCount.value === 0) return true;
      showDirtyModal({ kind: "close" });
      return false;
    };

    try {
      unlistenGoto = await listen("settings-goto", async (ev) => {
        const id = ev && ev.payload;
        if (id && tools.value.some((x) => x.id === id)) {
          await selectTool(id, { keepBanner: true });
        }
      });
    } catch {
      /* 事件注册失败不影响主流程 */
    }

    try {
      unlistenAppearance = await listen("appearance:changed", (ev) => {
        const p = (ev && ev.payload) || {};
        if (typeof values.value.appearance_theme === "string") {
          values.value.appearance_theme = p.theme;
        }
        if (typeof values.value.accent_color === "string") {
          values.value.accent_color = p.accent;
        }
      });
    } catch {
      /* 没有外观事件也能用 */
    }
  }

  // 「有 N 项未保存」常驻；值改回原值 / 保存 / 放弃后自动收掉。
  watch(pendingCount, (n) => {
    if (n > 0) {
      setStatus(t("settings.dirty", { n }), "dirty");
    } else if (statusKind.value === "dirty") {
      setStatus("");
    }
  });

  onBeforeUnmount(() => {
    clearTimeout(statusTimer);
    clearTimeout(bannerTimer);
    for (const timer of Object.values(autoApplyTimers)) clearTimeout(timer);
    if (typeof unlistenGoto === "function") unlistenGoto();
    if (typeof unlistenAppearance === "function") unlistenAppearance();
    if (window.__beforeClose) window.__beforeClose = null;
  });

  return {
    tools,
    toolId,
    sections,
    tabId,
    values,
    pending,
    status,
    statusKind,
    banner,
    loading,
    dirtyModal,
    tool,
    section,
    fields,
    pendingCount,
    modalText,
    val,
    commit,
    selectTool,
    toggleToolEnabled,
    saveAll,
    discardAll,
    resolveDirty,
    refreshAfterSave,
    showBanner,
    showToast,
    init,
  };
}
