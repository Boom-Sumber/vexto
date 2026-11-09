// 新建下载任务窗口。
//
// 与列表窗口是两个独立 WebviewWindow：本窗口点「新建」成功后由后端关掉自己
// 并打开列表；列表页点「+」则两窗并存（用户明确要求的交互）。
//
// 重命名只在单链接时可用：多个链接的文件名各不相同，一个名字框没法表达。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

let defaultDir = "";
let submitting = false;

function errText(e) {
  if (e == null) return "未知错误";
  if (typeof e === "string") return e;
  if (e.message) return String(e.message);
  return String(e);
}

function showError(msg) {
  const box = el("error");
  box.textContent = msg || "";
  box.classList.toggle("hidden", !msg);
}

function splitLinks() {
  return el("links").value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/// 多行链接时「重命名」没意义，禁用并解释原因。
function syncNameState() {
  const n = splitLinks().length;
  const nameEl = el("name");
  nameEl.disabled = n > 1;
  nameEl.placeholder = n > 1
    ? "多个链接时按各自文件名保存"
    : "选填：留空按链接里的文件名保存";
  if (n > 1) nameEl.value = "";
}

function fillPrefill(text) {
  if (!text) return;
  el("links").value = text;
  syncNameState();
  el("links").focus();
  el("links").setSelectionRange(text.length, text.length);
}

async function submit() {
  if (submitting) return;
  const links = splitLinks();
  if (!links.length) {
    showError("至少填一个下载链接");
    return;
  }

  const base = {
    name: el("name").value.trim(),
    dir: el("dir").value.trim(),
    max_speed_kb: Number(el("speed").value) || 0,
    connections: Number(el("conn").value) || 0,
    user_agent: el("ua").value.trim(),
    referer: el("referer").value.trim(),
  };
  const tasks = links.map((uri) => ({ uri, ...base }));

  submitting = true;
  el("submit").disabled = true;
  el("status").textContent = "正在创建…";
  showError("");
  try {
    const r = await invoke("download_add", { tasks });
    const failed = Array.isArray(r.failed) ? r.failed : [];
    if (failed.length) {
      showError(failed.map((f) => f.uri + "：" + f.error).join("\n"));
      el("status").textContent = `成功 ${r.ok} 个，失败 ${failed.length} 个`;
      // 只把失败的留在框里，改完可以再点一次
      el("links").value = failed.map((f) => f.uri).join("\n");
      syncNameState();
    } else {
      el("status").textContent = `已创建 ${r.ok} 个任务`;
      // 这个命令会销毁本窗口，invoke 的响应可能收不到：
      // 不 await，也别让 rejection 冒成「创建失败」的错误横幅。
      invoke("new_task_done").catch(() => {});
    }
  } catch (e) {
    showError("创建任务失败：" + errText(e));
    el("status").textContent = "";
  } finally {
    submitting = false;
    el("submit").disabled = false;
  }
}

(async function init() {
  try {
    defaultDir = await invoke("download_default_dir");
  } catch (e) {
    defaultDir = "";
  }
  el("dir").placeholder = defaultDir
    ? "留空则用：" + defaultDir
    : "留空用系统「下载」文件夹";

  el("links").addEventListener("input", syncNameState);
  el("resetDir").addEventListener("click", () => {
    el("dir").value = "";
    el("dir").placeholder = defaultDir
      ? "留空则用：" + defaultDir
      : "留空用系统「下载」文件夹";
  });
  el("cancel").addEventListener("click", () => {
    invoke("close_new_task").catch(() => {});
  });
  el("submit").addEventListener("click", submit);
  el("links").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) {
      ev.preventDefault();
      submit();
    }
  });

  // 窗口已存在时后端不会重新加载页面，只能靠事件补发新链接。
  try {
    await listen("new-task-prefill", (ev) => fillPrefill(ev.payload));
  } catch (e) {
    showError("订阅预填事件失败：" + errText(e));
  }

  try {
    fillPrefill(await invoke("take_new_task_prefill"));
  } catch (e) {
    /* 没有预填不是错误 */
  }

  syncNameState();
  el("links").focus();
})();
