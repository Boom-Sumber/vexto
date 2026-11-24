<script setup>
/* 翻译弹窗前端。
 *
 * 两种模式共用一个窗口，靠 payload 的 `mode` 区分：
 *   - `select`（划词翻译）：窗口不聚焦，保住用户选中的文字；源文本只读展示
 *   - `input`（输入翻译）：窗口可聚焦，源文本框是第一焦点，回车即翻译
 * 窗口的 focusable 由后端按 mode 切换（见 src-tauri/src/popup.rs）。
 *
 * 渲染必须**先复位再分支**：错误重试时如果不清上一轮状态，
 * 会出现"错误卡片上还挂着上次的译文"（S1b 踩过的坑）。
 */
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import Icon from "../components/Icon.vue";
import { useTheme } from "../composables/useTheme.js";
import { t } from "../i18n/index.js";

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;
const theme = useTheme();

const cardEl = ref(null);
const srcEl = ref(null);
const mode = ref("select");
const pinned = ref(false);
const engines = ref([]);
const currentEngine = ref("");
const src = ref("");
const lang = ref(t("popup.auto"));
const engineName = ref(t("popup.engineUnset"));
const loading = ref(false);
const dst = ref("");
const error = ref("");
const actionsVisible = ref(false);
const menuOpen = ref(false);
const translating = ref(false);
const copySrcIcon = ref("copy");
const copyDstLabel = ref(t("popup.copyDst"));

let lastPayload = null;
let hideTimer = 0;
let copyIconTimer = 0;
let copyLabelTimer = 0;
let typingMode = false;
let unlistenRender = null;
let unlistenFocus = null;

function timeoutMs() {
  const value = lastPayload && lastPayload.timeout;
  return Number.isFinite(value) && value > 0 ? value : 15000;
}

function closePopup() {
  clearTimeout(hideTimer);
  invoke("close_popup").catch(() => {});
}

/** 排定自动关闭。钉住时永不排定。 */
function scheduleHide(ms) {
  clearTimeout(hideTimer);
  if (pinned.value) return; // 钉住了就不许自己消失
  hideTimer = setTimeout(closePopup, ms || timeoutMs());
}

/** 量卡片高度告诉后端（ADR-011：原生窗口尺寸只能后端设）。 */
function reportHeight() {
  if (!cardEl.value) return;
  const r = cardEl.value.getBoundingClientRect();
  const style = getComputedStyle(cardEl.value);
  const mt = parseFloat(style.marginTop) || 0;
  const mb = parseFloat(style.marginBottom) || 0;
  const ml = parseFloat(style.marginLeft) || 0;
  const mr = parseFloat(style.marginRight) || 0;
  const h = Math.ceil(r.height + mt + mb);
  const w = Math.ceil(r.width + ml + mr);
  invoke("resize_popup", { height: Math.max(60, h), width: Math.max(120, w) }).catch(() => {});
}

/** 等 Vue 提交 DOM 再量；隐藏窗口里 rAF 可能被暂停，用定时器兜底。 */
function afterLayout(fn) {
  nextTick(() => {
    let done = false;
    const run = () => {
      if (done) return;
      done = true;
      fn();
    };
    requestAnimationFrame(() => requestAnimationFrame(run));
    setTimeout(run, 60);
  });
}

/** 源文本框按内容撑高（最多 max-height，超出滚动）。 */
function autoGrowSrc() {
  if (!srcEl.value) return;
  srcEl.value.style.height = "auto";
  srcEl.value.style.height = Math.min(srcEl.value.scrollHeight, 96) + "px";
}

/** 复位到「新一轮」的样子。 */
function resetCard() {
  menuOpen.value = false;
  loading.value = false;
  dst.value = "";
  error.value = "";
  actionsVisible.value = false;
  copySrcIcon.value = "copy";
  copyDstLabel.value = t("popup.copyDst");
}

function render(p) {
  lastPayload = p;
  mode.value = p.mode || "select";
  pinned.value = !!p.pinned;
  engines.value = Array.isArray(p.engines) ? p.engines : [];
  currentEngine.value = p.engine || "";

  // 先复位再分支：不复位的话「error → 重译成功」会同时显示错误框和译文
  // （S1b 踩过的坑，注释见文件头）。
  resetCard();

  // 源文本：**输入模式下永远不覆盖**。用户正在打的东西是权威，
  // 后端在 loading 时发的是空串，一旦写回去用户就发现自己打的字没了。
  // 划词模式下源文本来自后端取词，才需要回填。
  if (mode.value !== "input") {
    if (typeof p.src === "string" && src.value !== p.src) src.value = p.src;
  } else if (p.fresh) {
    // 重新按了一次输入翻译热键 = 要一个干净的框
    src.value = "";
  } else if (p.state === "done" && typeof p.src === "string" && p.src) {
    src.value = p.src;
  }

  nextTick(autoGrowSrc);
  lang.value = (p.dir && p.dir.from) || p.from || t("popup.auto");
  engineName.value = p.engineName || t("popup.engineUnset");

  if (p.state === "loading") {
    loading.value = true;
    actionsVisible.value = false;
    scheduleHide(Math.max(60000, timeoutMs()));
  } else if (p.state === "done") {
    dst.value = p.dst || "";
    actionsVisible.value = true;
    scheduleHide();
  } else if (p.state === "error") {
    error.value = p.message || t("popup.failed");
    actionsVisible.value = true;
    // 错误卡片留久一点，用户要读原因
    scheduleHide(Math.max(30000, timeoutMs() * 2));
  }

  afterLayout(reportHeight);
}

function setPinned(on) {
  pinned.value = on;
  if (on) {
    clearTimeout(hideTimer);
  } else if (lastPayload && lastPayload.state === "done") {
    // 取消钉住后重新开始倒计时，否则它会一直挂着
    scheduleHide();
  }
  invoke("set_pinned", { pinned: on }).catch(() => {});
}

function togglePin() {
  setPinned(!pinned.value);
}

function toggleEngineMenu() {
  if (!engines.value.length) return;
  menuOpen.value = !menuOpen.value;
  // 菜单在文档流里，卡片自己就变高了，这里只是让窗口跟着长 / 缩
  afterLayout(reportHeight);
}

function hideEngineMenu() {
  if (!menuOpen.value) return;
  menuOpen.value = false;
  afterLayout(reportHeight);
}

function chooseEngine(e) {
  hideEngineMenu();
  if (e.id === currentEngine.value) return;
  currentEngine.value = e.id;
  engineName.value = e.name;
  // 切换引擎写进配置，然后**用当前源文本重翻一次**。
  // 用户点引擎的意图就是"我要用这个引擎翻这段"，不重翻等于什么都没发生。
  invoke("switch_engine", { engineId: e.id })
    .then((res) => {
      if (res && res.ok === false) return;
      doTranslate();
    })
    .catch(() => {});
}

function doTranslate() {
  const text = src.value.trim();
  if (!text) return;
  clearTimeout(hideTimer);
  translating.value = true;
  invoke("translate_text", { text, mode: mode.value })
    .catch((e) => {
      // 命令本身报错（不是翻译失败）也要让用户看到
      lastPayload = { state: "error", message: String(e), mode: mode.value };
      render(lastPayload);
    })
    .finally(() => {
      translating.value = false;
    });
}

async function writeClipboard(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // 不可聚焦的窗口里 clipboard API 可能被拒，交给 Rust 侧兜底
    invoke("copy_text", { text }).catch(() => {});
  }
}

async function copySource() {
  const text = src.value;
  if (!text) return;
  await writeClipboard(text);
  copySrcIcon.value = "check";
  clearTimeout(copyIconTimer);
  copyIconTimer = setTimeout(() => {
    copySrcIcon.value = "copy";
  }, 1200);
}

async function copyDst() {
  const text = lastPayload && lastPayload.dst;
  if (!text) return;
  await writeClipboard(text);
  copyDstLabel.value = t("popup.copied");
  clearTimeout(copyLabelTimer);
  copyLabelTimer = setTimeout(() => {
    copyDstLabel.value = t("popup.copyDst");
  }, 1200);
}

function openSettings() {
  invoke("open_settings_from_popup").catch(() => {});
}

/** 拖动：标题栏按下交给系统移动窗口，源文本框里不能触发。 */
function onTitlebarMouseDown(e) {
  if (e.button !== 0) return;
  if (e.target.closest("#pin, #close")) return;
  invoke("start_dragging").catch(() => {});
}

/** 划词模式点进源文本框才切到可聚焦；只做一次。 */
function onSrcMouseDown() {
  if (mode.value === "input" || typingMode) return;
  typingMode = true;
  invoke("focus_for_typing").catch(() => {});
}

function onSrcKeydown(e) {
  if (e.key !== "Enter") return;
  // Shift+Enter 换行，单回车翻译
  if (e.shiftKey) return;
  e.preventDefault();
  doTranslate();
}

function enterInputMode() {
  typingMode = true;
  const el = srcEl.value;
  if (!el) return;
  el.focus();
  const v = src.value;
  el.setSelectionRange(v.length, v.length);
}

function onCardEnter() {
  clearTimeout(hideTimer);
}

function onCardLeave() {
  if (lastPayload && lastPayload.state === "done") scheduleHide();
}

function onDocClick(e) {
  if (!e.target.closest("#engine-row, #engine-menu")) hideEngineMenu();
}

function focusSrc() {
  const el = srcEl.value;
  if (!el) return;
  el.focus();
  const v = src.value;
  el.setSelectionRange(v.length, v.length);
}

const listening = listen("popup-render", (e) => {
  render(e.payload || {});
  if ((e.payload && e.payload.mode) === "input") enterInputMode();
});

onMounted(async () => {
  document.addEventListener("click", onDocClick);
  try {
    await theme.init();
  } catch {
    /* 保持 theme.css 默认外观 */
  }
  try {
    unlistenFocus = await listen("popup-focus", focusSrc);
  } catch {
    /* 焦点事件注册失败不影响主流程 */
  }
  try {
    // 必须等 listen 注册完成再通知后端；否则补发的 popup-render 会丢。
    unlistenRender = await listening;
  } catch (e) {
    console.error("popup-render 监听注册失败", e);
  }
  invoke("popup_ready").catch(() => {});
  if (mode.value === "input") enterInputMode();
  afterLayout(reportHeight);
});

onBeforeUnmount(() => {
  clearTimeout(hideTimer);
  clearTimeout(copyIconTimer);
  clearTimeout(copyLabelTimer);
  document.removeEventListener("click", onDocClick);
  if (typeof unlistenFocus === "function") unlistenFocus();
  if (typeof unlistenRender === "function") unlistenRender();
});
</script>

<template>
  <div id="card" ref="cardEl" @mouseenter="onCardEnter" @mouseleave="onCardLeave">
    <!-- 标题栏：只放图钉和关闭。按住这里可以拖动整个弹窗 -->
    <div id="titlebar" @mousedown="onTitlebarMouseDown">
      <button
        id="pin"
        class="icon"
        type="button"
        :title="pinned ? $t('popup.pinned') : $t('popup.pin')"
        :aria-pressed="String(pinned)"
        @click="togglePin"
      >
        <Icon name="pin" :size="14" />
      </button>
      <span id="title-spacer"></span>
      <button id="close" class="icon" type="button" :title="$t('common.close')" @click="closePopup">
        <Icon name="close" :size="14" />
      </button>
    </div>

    <!-- 源文本：两种模式都可编辑，但划词模式点进来才抢焦点（见 popup.rs 注释） -->
    <div id="src-wrap">
      <textarea
        id="src"
        ref="srcEl"
        v-model="src"
        rows="1"
        spellcheck="false"
        :placeholder="$t('popup.srcPlaceholder')"
        @input="autoGrowSrc"
        @mousedown="onSrcMouseDown"
        @keydown="onSrcKeydown"
      ></textarea>
    </div>

    <!-- 源文本工具栏：复制原文 / 语种胶囊 / 翻译按钮 -->
    <div id="src-tools">
      <button
        v-show="mode !== 'input'"
        id="copy-src"
        class="icon"
        type="button"
        :title="$t('popup.copySource')"
        @click="copySource"
      >
        <Icon :name="copySrcIcon" :size="14" />
      </button>
      <span id="lang-pill" class="pill" :title="$t('popup.langTitle')">{{ lang }}</span>
      <span id="tools-spacer"></span>
      <button id="translate-btn" class="primary" type="button" :disabled="translating" @click="doTranslate">
        {{ $t("popup.translate") }}
      </button>
    </div>

    <!-- 引擎选择：固定宽度的小选择框（不带下拉箭头）。
         点它弹出引擎菜单，菜单在文档流里，卡片随之变高。 -->
    <button id="engine-row" type="button" :title="$t('popup.engineTitle')" @click.stop="toggleEngineMenu">
      <span id="engine-name">{{ engineName }}</span>
    </button>
    <div id="engine-menu" v-show="menuOpen">
      <button
        v-for="e in engines"
        :key="e.id"
        class="engine-item"
        :class="{ on: e.id === currentEngine }"
        @click="chooseEngine(e)"
      >
        {{ e.name }}
      </button>
    </div>

    <div id="divider"></div>

    <div id="loading" class="loading" v-show="loading">
      <span class="dot"></span><span class="dot"></span><span class="dot"></span>
      <span class="loading-text">{{ $t("popup.loading") }}</span>
    </div>

    <div id="dst" class="dst" v-show="dst">{{ dst }}</div>
    <div id="error" class="error" v-show="error">{{ error }}</div>

    <div id="actions" :class="{ on: actionsVisible }">
      <button id="copy-dst" class="icon-text" type="button" :title="$t('popup.copyDst')" @click="copyDst">
        <Icon name="copy" :size="14" /><span>{{ copyDstLabel }}</span>
      </button>
      <span id="actions-spacer"></span>
      <button id="open-settings" class="icon" type="button" :title="$t('common.settings')" @click="openSettings">
        <Icon name="gear" :size="16" />
      </button>
    </div>
  </div>
</template>
