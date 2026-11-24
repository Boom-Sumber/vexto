<script setup>
// 新建任务窗口（按 Rayburst 参考界面还原）。
//
// **本轮只有「链接任务」一个可用页签**：种子任务 / 磁力按用户要求先关闭，
// 按钮留在界面上但 disabled（后端 `addTorrent` 与校验都还在，开放时删掉
// disabled 即可）。
// 存储路径 / 可用空间 / 高级选项是共享区。
// 单链接时自动探测文件名与大小；多链接时名字框留空、只提示「选填」，
// 「重新获取」禁用——一个名字框表达不了多个不同文件名。
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import TitleBar from "../components/TitleBar.vue";
import Icon from "../components/Icon.vue";
import { vNumField } from "../directives/numField.js";

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const tb = ref(null);
const linksEl = ref(null);
const torrentFileEl = ref(null);

const links = ref("");
const name = ref("");
const nameEdited = ref(false);
const nameSizeText = ref("");
const lastSize = ref(0);
const dir = ref("");
const defaultDir = ref("");
const speed = ref(0);
const conn = ref(0);
const ua = ref("");
const uaEdited = ref(false);
const referer = ref("");
const checksum = ref("");
const httpUser = ref("");
const httpPasswd = ref("");
const authorization = ref("");
const advOpen = ref(false);
const activeTab = ref("link");
const torrentB64 = ref("");
const torrentBytes = ref(0);
const torrentName = ref("");
const torrentSizeText = ref("");
const error = ref("");
const tError = ref("");
const status = ref("");
const submitting = ref(false);
const spaceText = ref("");

// 浏览器扩展交过来的附加信息：url → { filename, size, referer, userAgent, cookie }。
// 热键预填只有 url，不会有这些字段。
const extras = new Map();
let probeSeq = 0;
let probeTimer = null;

// 与后端 `validate::MAX_TORRENT_B64` 对应的原文件上限（约 4 MB）。
const MAX_TORRENT_FILE = 4 * 1024 * 1024;

const linkList = computed(() =>
  links.value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean),
);
const multi = computed(() => linkList.value.length > 1);
const reprobeDisabled = computed(() => multi.value || linkList.value.length === 0);
// 后端以 Authorization 为准、不发 Basic（ADR-024）。两组都填时界面必须说明，
// 否则用户会以为用户名密码也生效了。
const authHint = computed(
  () => authorization.value.trim() !== "" && (httpUser.value.trim() !== "" || httpPasswd.value !== ""),
);

// UA 文本框允许视觉折行；用户真敲进去的回车不能带进 aria2 的 header，
// 提交前把换行连同它后面的空白一起吃掉，让折开的两截重新拼回一个 UA。
function cleanUa(s) {
  return String(s || "").replace(/[\r\n]+\s*/g, "").trim();
}

function fmtBytes(n) {
  n = Number(n) || 0;
  if (!n) return "";
  if (n < 1024) return n + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < u.length - 1);
  // 统一 1 位小数：153.3 GB / 1.3 KB / 12 MB（去掉无意义的 .0）
  return n.toFixed(1).replace(/\.0$/, "") + " " + u[i];
}

function errText(e) {
  if (e == null) return "未知错误";
  if (typeof e === "string") return e;
  if (e.message) return String(e.message);
  return String(e);
}

function showError(msg, which) {
  if (which === "torrent") tError.value = msg || "";
  else error.value = msg || "";
}

function clearErrors() {
  showError("");
  showError("", "torrent");
}

function splitLinks() {
  return linkList.value;
}

/// 前端先按 URL 猜一个名字，别让用户盯着空框等网络。
function guessName(url) {
  try {
    const u = new URL(url);
    const seg = u.pathname.split("/").filter(Boolean).pop() || "";
    return seg ? decodeURIComponent(seg) : "";
  } catch {
    return "";
  }
}

function setTab(which) {
  // 种子任务页签暂时关闭：任何路径（脚本、键盘）都退回链接页签，
  // 否则会出现「界面没有种子页、activeTab 却是 torrent」的错位提交。
  if (which === "torrent") return;
  activeTab.value = "link";
  clearErrors();
  updateSpaceHint();
}

/// 多链接时名字框清空并只显示「选填」提示，「重新获取」禁用。
function syncNameState() {
  if (multi.value) {
    name.value = "";
    nameSizeText.value = "";
    checksum.value = "";
    lastSize.value = 0;
  }
}

async function updateSpaceHint(size) {
  if (size !== undefined) lastSize.value = size;
  const d = dir.value.trim() || defaultDir.value;
  let free = null;
  try {
    free = await invoke("disk_free", { dir: d });
  } catch {
    free = null;
  }
  const parts = [];
  if (activeTab.value === "torrent") {
    // 种子文件本身的大小 ≠ 任务总大小（要解析 bencode 才知道），
    // 所以只如实写「种子 x KB」，不冒充「需下载 x」。
    if (torrentBytes.value) parts.push("种子 " + fmtBytes(torrentBytes.value));
  } else if (lastSize.value) {
    parts.push("需下载 " + fmtBytes(lastSize.value));
  }
  if (free) parts.push("可用 " + fmtBytes(free));
  spaceText.value = parts.join("，");
}

/// 单链接时探测文件名与大小。`force` 表示用户点了「重新获取」。
async function probeNow(force) {
  const list = splitLinks();
  if (list.length !== 1) {
    lastSize.value = 0;
    nameSizeText.value = "";
    updateSpaceHint();
    return;
  }

  if (force) {
    nameEdited.value = false;
    name.value = "";
  }
  if (!name.value && !nameEdited.value) {
    name.value = guessName(list[0]);
  }

  const seq = ++probeSeq;
  nameSizeText.value = "";
  lastSize.value = 0;
  updateSpaceHint();

  try {
    const r = await invoke("download_probe", { url: list[0] });
    if (seq !== probeSeq) return; // 已有更新的探测，丢掉旧结果
    if (r && r.name && !nameEdited.value) {
      name.value = r.name;
    }
    if (r && r.size) {
      nameSizeText.value = fmtBytes(r.size);
      updateSpaceHint(r.size);
    }
  } catch {
    /* 探测失败不影响手填，也不弹错误横幅 */
  }
}

function scheduleProbe() {
  clearTimeout(probeTimer);
  probeTimer = setTimeout(() => probeNow(false), 450);
}

/// Uint8Array → base64。分块避免 `apply` 一次塞太多参数导致栈溢出。
function bytesToBase64(bytes) {
  let s = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    s += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
  }
  return btoa(s);
}

function resetTorrent() {
  torrentB64.value = "";
  torrentBytes.value = 0;
  torrentName.value = "";
  torrentSizeText.value = "";
  updateSpaceHint();
}

/// 只做两道轻量校验：大小上限 + bencode 顶层必须是 `d…e` 字典。
/// 真正的种子解析交给 aria2（它会给出明确的 RPC 错误），前端不引 bencode 库。
async function onTorrentPicked(ev) {
  const file = ev.target.files && ev.target.files[0];
  if (!file) return;
  // 允许连续选同一个文件（否则第二次 change 不触发）
  ev.target.value = "";
  showError("", "torrent");

  if (file.size > MAX_TORRENT_FILE) {
    resetTorrent();
    showError("种子文件过大（上限 4 MB）", "torrent");
    return;
  }
  let bytes;
  try {
    bytes = new Uint8Array(await file.arrayBuffer());
  } catch (e) {
    resetTorrent();
    showError("读取种子文件失败：" + errText(e), "torrent");
    return;
  }
  if (bytes.length < 2 || bytes[0] !== 0x64 || bytes[bytes.length - 1] !== 0x65) {
    resetTorrent();
    showError("这不像是 .torrent 文件（内容应为 bencode 字典 d…e）", "torrent");
    return;
  }

  torrentB64.value = bytesToBase64(bytes);
  torrentBytes.value = bytes.length;
  torrentName.value = file.name;
  torrentSizeText.value = fmtBytes(bytes.length);
  updateSpaceHint();
}

async function submit() {
  if (submitting.value) return;
  return activeTab.value === "torrent" ? submitTorrent() : submitLinks();
}

async function submitLinks() {
  const list = splitLinks();
  if (!list.length) {
    showError("至少填一个下载链接");
    return;
  }

  const single = list.length === 1;
  const base = {
    dir: dir.value.trim() || defaultDir.value,
    max_speed_kb: Number(speed.value) || 0,
    connections: Number(conn.value) || 0,
    user_agent: cleanUa(ua.value),
    referer: referer.value.trim(),
    // 校验和只对单条链接有意义：多条链接内容是不同文件，不可能共用一个摘要。
    // 后端 `validate::checksum_option` 会再校验格式，这里只在多链接时丢弃。
    checksum: single ? checksum.value.trim() : "",
    http_user: httpUser.value.trim(),
    http_passwd: httpPasswd.value,
    // 原始 Authorization 头。与上面两项同属「认证信息」区块，同时填时后端
    // 只发这个（ADR-024）；界面上的 authHint 已经提前告知。
    authorization: authorization.value.trim(),
  };
  const typedName = single ? name.value.trim() : "";
  const tasks = list.map((uri) => {
    const ex = extras.get(uri) || {};
    return {
      uri,
      dir: base.dir,
      max_speed_kb: base.max_speed_kb,
      connections: base.connections,
      // 优先级：用户在框里改的 > 扩展给的原文件名 > 空（后端从 URL 推）
      name: typedName || ex.filename || "",
      // UA / Referer 以用户手填为准；没填则用扩展带来的（防盗链/登录态需要）
      user_agent: base.user_agent || ex.userAgent || "",
      referer: base.referer || ex.referer || "",
      // Cookie 不上界面，直接随任务下发（扩展抢来的登录态）
      cookie: ex.cookie || "",
      checksum: base.checksum,
      http_user: base.http_user,
      http_passwd: base.http_passwd,
      authorization: base.authorization,
    };
  });

  submitting.value = true;
  status.value = "正在创建…";
  showError("");
  try {
    const r = await invoke("download_add", { tasks });
    const failed = Array.isArray(r.failed) ? r.failed : [];
    if (failed.length) {
      showError(failed.map((f) => f.uri + "：" + f.error).join("\n"));
      status.value = `成功 ${r.ok} 个，失败 ${failed.length} 个`;
      links.value = failed.map((f) => f.uri).join("\n");
      syncNameState();
      scheduleProbe();
    } else {
      status.value = `已创建 ${r.ok} 个任务`;
      // 这个命令会销毁本窗口，响应可能收不到：不 await，也别让 rejection 冒成错误
      invoke("new_task_done").catch(() => {});
    }
  } catch (e) {
    showError("创建任务失败：" + errText(e));
    status.value = "";
  } finally {
    submitting.value = false;
  }
}

async function submitTorrent() {
  if (!torrentB64.value) {
    showError("请先选择一个 .torrent 文件", "torrent");
    return;
  }
  const task = {
    torrent_base64: torrentB64.value,
    // 名字只用于设置页状态行回显；aria2 用种子里的 info.name，不接受覆盖。
    name: torrentName.value,
    dir: dir.value.trim() || defaultDir.value,
    max_speed_kb: Number(speed.value) || 0,
  };

  submitting.value = true;
  status.value = "正在创建…";
  showError("", "torrent");
  try {
    await invoke("download_add_torrent", { task });
    status.value = "已创建 1 个种子任务";
    invoke("new_task_done").catch(() => {});
  } catch (e) {
    showError("创建种子任务失败：" + errText(e), "torrent");
    status.value = "";
  } finally {
    submitting.value = false;
  }
}

/// 追加预填条目。热键只给一条纯 URL；浏览器扩展会给 url + 文件名/大小/cookie。
/// 窗口已开着时**追加**到输入框末尾，不覆盖用户已经改过的内容。
function addPrefill(payload) {
  const list = Array.isArray(payload) ? payload : payload ? [payload] : [];
  if (!list.length) return;

  const known = new Set(splitLinks());
  const fresh = [];
  for (const raw of list) {
    const item = typeof raw === "string" ? { url: raw } : raw || {};
    const url = String(item.url || "").trim();
    if (!url || known.has(url)) continue;
    known.add(url);
    fresh.push(url);
    extras.set(url, {
      filename: String(item.filename || "").trim(),
      size: Number(item.size) || 0,
      referer: String(item.referer || "").trim(),
      userAgent: String(item.user_agent || item.userAgent || "").trim(),
      cookie: String(item.cookie || ""),
    });
  }
  if (!fresh.length) return;

  // 热键 / 扩展送来的都是链接，别让它们填进一个当前不可见的输入框。
  setTab("link");

  const cur = links.value.trim();
  links.value = cur ? cur + "\n" + fresh.join("\n") : fresh.join("\n");

  const now = splitLinks();
  const ex = now.length === 1 ? extras.get(now[0]) || {} : null;
  nameEdited.value = false;
  if (ex && ex.filename) {
    name.value = ex.filename;
    // 别让自动探测把扩展给的原文件名覆盖掉；用户仍可点「重新获取」。
    nameEdited.value = true;
    nameSizeText.value = ex.size ? fmtBytes(ex.size) : "";
  }
  // 接管带来的 UA 回显（用户要求）：只写单链接——多链接的 UA 可能各不相同，
  // 塞进同一个框会把第一条的 UA 应用到全部任务（提交时本来就是按链取 ex.userAgent）。
  // 用户手动改过 UA 的话不覆盖。
  if (ex && ex.userAgent && !uaEdited.value) {
    ua.value = ex.userAgent;
    advOpen.value = true;
  }
  syncNameState();

  // 扩展给了大小时不再探测：探测失败会把刚显示的大小清空。
  if (ex && ex.size) updateSpaceHint(ex.size);
  else scheduleProbe();

  nextTick(() => {
    linksEl.value?.focus();
    const v = links.value;
    linksEl.value?.setSelectionRange(v.length, v.length);
  });
}

function onLinksInput() {
  nameEdited.value = false;
  syncNameState();
  scheduleProbe();
}

function onNameInput() {
  nameEdited.value = true;
}

function onUaInput() {
  uaEdited.value = true;
}

function openDir() {
  invoke("open_path", { path: dir.value.trim() || defaultDir.value }).catch(() => {});
}

function useDefaultDir() {
  dir.value = defaultDir.value;
  updateSpaceHint();
}

function pickTorrent() {
  torrentFileEl.value?.click();
}

function cancel() {
  invoke("close_new_task").catch(() => {});
}

onMounted(async () => {
  // 限速 / 连接数 / UA 的默认值来自设置（会话 25 用户要求）。
  // 一次取回：串四个 IPC 会让窗口先画出空框再一个个跳值。
  try {
    const d = await invoke("download_task_defaults");
    defaultDir.value = d.dir || "";
    // 0 = 不限速 / 不限连接，照实显示 0，不留空框
    speed.value = Number(d.speed ?? 0);
    conn.value = Number(d.connections ?? 0);
    // UA 为空表示「用 aria2 自带 UA」：框里留空，不冒充浏览器
    ua.value = d.ua || "";
  } catch {
    defaultDir.value = "";
  }
  // 存储路径直接写值（不是占位符）：用户要求打开就能看到具体路径
  dir.value = defaultDir.value;

  try {
    // 事件只是通知：数据统一由 take_new_task_prefill 取走，避免窗口刚建、
    // 监听还没注册时事件丢失（连续两个下载会一起丢）。
    await listen("new-task-prefill", async () => {
      try {
        addPrefill(await invoke("take_new_task_prefill"));
      } catch {
        /* 取不到不是错误 */
      }
    });
  } catch (e) {
    showError("订阅预填事件失败：" + errText(e));
  }

  try {
    addPrefill(await invoke("take_new_task_prefill"));
  } catch {
    /* 没有预填不是错误 */
  }

  syncNameState();
  updateSpaceHint();
  nextTick(() => {
    linksEl.value?.focus();
    const v = links.value;
    linksEl.value?.setSelectionRange(v.length, v.length);
  });
  // 默认值 / 预填都上屏了，再让后端显示窗口（会话 32 修白屏）
  await nextTick();
  tb.value?.reveal();
});

onBeforeUnmount(() => {
  clearTimeout(probeTimer);
});
</script>

<template>
  <div class="win">
    <!-- 固定尺寸窗口：只留主题 / 最小化 / 关闭，不装缩放热区、不显示最大化。 -->
    <TitleBar ref="tb" title="新建任务" :resize="false" :maximize="false" />

    <nav class="tabs">
      <button id="tabLink" class="tab" :class="{ active: activeTab === 'link' }" type="button" @click="setTab('link')">
        链接任务
      </button>
      <!-- 种子任务先关闭：BT / 磁力还没开放（会话 25 用户要求）。
           按钮保留在设计里，等集成时删掉 disabled 即可，后端命令与校验都还在。 -->
      <button
        id="tabTorrent"
        class="tab"
        :class="{ active: activeTab === 'torrent' }"
        type="button"
        disabled
        title="种子与磁力下载将在后续版本开放"
      >
        种子任务
      </button>
    </nav>

    <main class="body">
      <div id="paneLink" class="pane" v-show="activeTab === 'link'">
        <textarea
          id="links"
          ref="linksEl"
          v-model="links"
          class="textarea"
          spellcheck="false"
          placeholder="每行一个下载链接"
          @input="onLinksInput"
          @keydown.ctrl.enter.prevent="submit"
          @keydown.meta.enter.prevent="submit"
        ></textarea>

        <div id="error" class="notice notice-danger error" v-show="error">{{ error }}</div>

        <div class="field">
          <label class="lbl" for="name">重命名：</label>
          <div class="input-wrap">
            <input
              id="name"
              v-model="name"
              class="input has-suffix"
              type="text"
              spellcheck="false"
              placeholder="选填"
              @input="onNameInput"
            />
            <span id="nameSize" class="suffix num">{{ nameSizeText }}</span>
          </div>
          <button id="reprobe" class="btn" type="button" :disabled="reprobeDisabled" @click="probeNow(true)">
            重新获取
          </button>
        </div>
      </div>

      <div id="paneTorrent" class="pane" v-show="activeTab === 'torrent'">
        <div class="field">
          <label class="lbl" for="torrentName">种子文件：</label>
          <div class="input-wrap">
            <input
              id="torrentName"
              :value="torrentName"
              class="input has-suffix"
              type="text"
              readonly
              placeholder="选择 .torrent 文件"
            />
            <span id="torrentSize" class="suffix num">{{ torrentSizeText }}</span>
          </div>
          <button id="pickTorrent" class="btn" type="button" @click="pickTorrent">选择…</button>
          <!-- 原生 file input 保持隐藏：系统部件是白底，只允许用自绘按钮触发它。
               文件读成 base64 后走 IPC，Rust 侧不需要文件权限 / dialog 插件。 -->
          <input
            id="torrentFile"
            ref="torrentFileEl"
            class="hidden"
            type="file"
            accept=".torrent,application/x-bittorrent"
            @change="onTorrentPicked"
          />
        </div>
        <div class="hint-row">种子里的文件名与分片校验由种子决定，这里只改保存位置与限速。</div>
        <div id="tError" class="notice notice-danger error" v-show="tError">{{ tError }}</div>
      </div>

      <!-- 两个页签共享：保存位置 / 可用空间 / 高级选项。
           链接任务下顺序与参考图一致（重命名 → 存储路径 → 高级选项）。 -->
      <div class="field">
        <label class="lbl" for="dir">存储路径：</label>
        <div class="input-wrap">
          <input id="dir" v-model="dir" class="input" type="text" spellcheck="false" @change="updateSpaceHint()" />
          <button id="pickDir" class="icon-btn dir-btn" type="button" title="在资源管理器中打开" aria-label="在资源管理器中打开" @click="openDir">
            <Icon name="folder" :size="16" />
          </button>
          <button id="defaultDir" class="icon-btn dir-btn" type="button" title="填入默认下载目录" aria-label="填入默认下载目录" @click="useDefaultDir">
            <Icon name="clock" :size="16" />
          </button>
        </div>
      </div>

      <div id="spaceHint" class="hint-row num">{{ spaceText }}</div>

      <label class="adv-toggle">
        <input id="advToggle" v-model="advOpen" type="checkbox" />
        <span>高级选项</span>
      </label>

      <div id="advPane" class="adv" v-show="advOpen">
        <div class="field">
          <label class="lbl" for="speed">限速：</label>
          <div class="input-wrap">
            <input
              id="speed"
              v-model.number="speed"
              v-num-field
              class="input has-suffix"
              type="number"
              min="0"
              max="102400"
              step="64"
              placeholder="0"
            />
            <span class="suffix">KB/s</span>
          </div>
        </div>
        <div class="field">
          <label class="lbl" for="conn">连接数：</label>
          <div class="input-wrap">
            <input
              id="conn"
              v-model.number="conn"
              v-num-field
              class="input has-suffix"
              type="number"
              min="0"
              max="16"
              step="1"
              placeholder="0"
            />
            <span class="suffix">个</span>
          </div>
        </div>
        <div class="field" v-show="activeTab === 'link'">
          <label class="lbl" for="ua">User-Agent：</label>
          <div class="input-wrap">
            <!-- UA 是一长串无空格字符，单行框看不全：用两行可折行的文本框。
                 视觉折行不进 value；手敲的回车由 cleanUa() 去掉。 -->
            <textarea
              id="ua"
              v-model="ua"
              class="input ua-input"
              rows="2"
              spellcheck="false"
              placeholder="选填"
              @input="onUaInput"
            ></textarea>
          </div>
        </div>
        <div class="field" v-show="activeTab === 'link'">
          <label class="lbl" for="referer">Referer：</label>
          <div class="input-wrap">
            <input id="referer" v-model="referer" class="input" type="text" spellcheck="false" placeholder="选填" />
          </div>
        </div>
        <div class="field" v-show="activeTab === 'link'">
          <label class="lbl" for="checksum">校验和：</label>
          <div class="input-wrap">
            <input
              id="checksum"
              v-model="checksum"
              class="input"
              type="text"
              spellcheck="false"
              :disabled="multi"
              :title="multi ? '多条链接不能共用一个校验和' : ''"
              placeholder="sha-256=…（选填）"
            />
          </div>
        </div>
        <div class="field auth" v-show="activeTab === 'link'">
          <!-- 三行合成一项：用户名/密码是 HTTP Basic 的两种字段，Authorization 是
               原始头。三者共用「认证信息」这个标签，label 因此不带 for（对应多个输入框）。 -->
          <span class="lbl">认证信息：</span>
          <div class="auth-body">
            <div class="auth-row">
              <input id="httpUser" v-model="httpUser" class="input" type="text" spellcheck="false" placeholder="用户名" />
              <input
                id="httpPasswd"
                v-model="httpPasswd"
                class="input"
                type="password"
                spellcheck="false"
                placeholder="密码"
              />
            </div>
            <div class="auth-row group">
              <span class="auth-prefix">Authorization</span>
              <input
                id="authorization"
                v-model="authorization"
                class="input"
                type="text"
                spellcheck="false"
                placeholder="Bearer 你的令牌"
              />
            </div>
            <div id="authHint" class="auth-hint" v-show="authHint">已填 Authorization，用户名 / 密码不会发送</div>
          </div>
        </div>
      </div>
    </main>

    <footer class="foot">
      <span id="status" class="status">{{ status }}</span>
      <div class="acts">
        <button id="cancel" class="btn" type="button" @click="cancel">取消</button>
        <button id="submit" class="btn btn-primary" type="button" :disabled="submitting" @click="submit">新建</button>
      </div>
    </footer>
  </div>
</template>
