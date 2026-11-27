import { computed, ref } from "vue";

/* 任务窗口侧栏的工具顺序与文案。不用 list_tools 的顺序/名称：
 * registry 里还有 General/About，工具名是后端本地化的长名，
 * 新增工具会自动进设置页但不一定有待接的主窗视图。 */
export const MAIN_TOOLS = [
  { id: "translate", icon: "translate", labelKey: "main.navTranslate" },
  { id: "screenshot", icon: "camera", labelKey: "main.navScreenshot" },
  { id: "download", icon: "download", labelKey: "main.navDownload" },
  { id: "transfer", icon: "transfer", labelKey: "main.navTransfer" },
];

/* 工具启用状态：list_tools 只用来取 enabled。
 * 拿不到时按全部启用处理；窗口重新获得焦点时重拉，设置窗改完切回即生效。 */
export function useTools() {
  const enabled = ref({});

  async function refresh() {
    try {
      const invoke = window.__TAURI__.core.invoke;
      const tools = await invoke("list_tools");
      enabled.value = Object.fromEntries(
        (tools || []).map((tool) => [tool.id, tool.enabled !== false]),
      );
    } catch {
      enabled.value = {};
    }
  }

  const visible = computed(() => MAIN_TOOLS.filter((tool) => enabled.value[tool.id] !== false));

  function isVisible(id) {
    return enabled.value[id] !== false;
  }

  return { refresh, visible, isVisible };
}
