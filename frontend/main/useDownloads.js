import { computed, ref, shallowRef } from "vue";
import { t } from "../i18n/index.js";
import { errText } from "../settings/utils.js";

// 下载视图：数据全部来自后端 `tasks:snapshot` 与 `download_snapshot`。
// 快照用 shallowRef：任务对象可能有上百条，不需要深响应式，整体替换即可。
export function useDownloads() {
  const invoke = window.__TAURI__.core.invoke;
  const listen = window.__TAURI__.event.listen;

  const tasks = shallowRef([]);
  const global = shallowRef({});
  const error = ref("");
  const filter = ref("all");
  // 后端的两条界面开关：默认「要确认、不删文件」，是最保守的假设。
  const uiFlags = ref({ confirm: true, delete_file: false, delete_mode: "recycle" });
  const loaded = ref(false);

  let unlisten = null;

  const titles = computed(() => ({
    all: t("main.titleAll"),
    running: t("main.titleRunning"),
    failed: t("main.titleFailed"),
    complete: t("main.titleComplete"),
    deleted: t("main.titleDeleted"),
    github: t("main.titleGithub"),
  }));

  // 空态图标（icons.js 注册名；会话 55：空态从纯文字升级为图标 + 文案）
  const emptyIcon = computed(() => ({
    all: "download",
    running: "bolt",
    failed: "alert-triangle",
    complete: "check-circle",
    deleted: "trash",
  }));

  const emptyText = computed(() => ({
    all: t("main.emptyAll"),
    running: t("main.emptyRunning"),
    failed: t("main.emptyFailed"),
    complete: t("main.emptyComplete"),
    deleted: t("main.emptyDeleted"),
  }));

  function catOf(task) {
    if (task.status === "complete") return task.file_missing ? "deleted" : "complete";
    if (task.status === "error" || task.status === "removed") return "failed";
    return "running";
  }

  // 主菜单「下载」上的徽标：进行中数量，0 时隐藏（会话 55，复用分类计数）
  const counts = computed(() => {
    const n = { all: tasks.value.length, running: 0, failed: 0, complete: 0, deleted: 0 };
    for (const task of tasks.value) n[catOf(task)] += 1;
    return n;
  });

  const visibleTasks = computed(() => {
    if (filter.value === "github") return [];
    if (filter.value === "all") return tasks.value;
    return tasks.value.filter((task) => catOf(task) === filter.value);
  });

  function showError(msg) {
    error.value = msg || "";
  }

  function render(snapshot) {
    if (!snapshot || !Array.isArray(snapshot.tasks)) return;
    tasks.value = snapshot.tasks;
    global.value = snapshot.global || {};
    loaded.value = true;
  }

  async function refresh() {
    try {
      render(await invoke("download_snapshot"));
    } catch (e) {
      showError(errText(e));
    }
  }

  async function call(cmd, args) {
    try {
      await invoke(cmd, args || {});
      showError("");
      await refresh();
    } catch (e) {
      showError(errText(e));
    }
  }

  function setFilter(cat) {
    filter.value = cat;
  }

  const pause = (gid) => call("download_pause", { gid });
  const resume = (gid) => call("download_unpause", { gid });
  const remove = (gid) => call("download_remove", { gid });
  const openDir = (gid) => call("download_open_dir", { gid });
  const clearFinished = () => call("download_clear_finished");

  async function newTask() {
    try {
      await invoke("open_new_task");
    } catch (e) {
      showError(errText(e));
    }
  }

  async function init() {
    try {
      uiFlags.value = await invoke("download_ui_flags");
    } catch {
      /* 读不到就用默认值 */
    }
    await refresh();
    try {
      unlisten = await listen("tasks:snapshot", (ev) => {
        try {
          render(ev.payload);
          if (error.value) showError("");
        } catch (e) {
          showError(t("main.snapshotFailed", { message: errText(e) }));
        }
      });
    } catch (e) {
      showError(t("main.subscribeFailed", { message: errText(e) }));
    }
  }

  function dispose() {
    if (typeof unlisten === "function") unlisten();
  }

  return {
    tasks,
    global,
    error,
    filter,
    uiFlags,
    loaded,
    titles,
    emptyIcon,
    emptyText,
    counts,
    visibleTasks,
    catOf,
    setFilter,
    refresh,
    pause,
    resume,
    remove,
    openDir,
    clearFinished,
    newTask,
    init,
    dispose,
  };
}
