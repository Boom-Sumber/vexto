<script setup>
// 全屏截图右下角缩略图反馈（S7 会话 72）。
// 倒计时、淡出、悬停暂停/恢复、复制/钉住/预览按钮；图片走 vextoimg 协议。
import { onBeforeUnmount, onMounted, ref } from "vue";
import Icon from "../components/Icon.vue";

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const url = ref("");
const seconds = ref(4);
const fadeMs = ref(400);
const closing = ref(false);
const started = ref(false);
let timer = 0;

const ACTS = [
  { act: "copy", icon: "copy", title: "复制图片" },
  { act: "folder", icon: "folder", title: "打开所在文件夹" },
  { act: "pin", icon: "pin", title: "钉住" },
  { act: "preview", icon: "preview", title: "预览" },
];

function schedule() {
  clearTimeout(timer);
  if (closing.value) return;
  timer = setTimeout(fade, seconds.value * 1000);
}

function pause() {
  clearTimeout(timer);
}

function fade() {
  if (closing.value) return;
  closing.value = true;
  timer = setTimeout(() => invoke("feedback_close").catch(() => {}), fadeMs.value + 30);
}

function close() {
  closing.value = true;
  clearTimeout(timer);
  invoke("feedback_close").catch(() => {});
}

function show(p) {
  if (!p || !p.url) {
    close();
    return;
  }
  seconds.value = Math.max(3, Math.min(5, Number(p.seconds) || 4));
  fadeMs.value = Math.max(200, Math.min(1500, Number(p.fadeMs) || 400));
  closing.value = false;
  url.value = p.url;
}

function onLoad() {
  invoke("window_ready").catch(() => {});
  if (!started.value) {
    // 第一次：窗口还是全屏，加载完后收缩到右下角。
    started.value = true;
    invoke("feedback_start_animation")
      .catch(() => {})
      .finally(schedule);
  } else {
    schedule();
  }
}

async function act(action) {
  try {
    const r = await invoke("feedback_action", { action });
    if (r && r.ok && (action === "pin" || action === "preview" || action === "folder")) {
      close();
    }
  } catch {
    // 失败不关窗，用户可以重试或直接等它淡出。
  }
}

function onKey(ev) {
  if (ev.key === "Escape") close();
}

onMounted(async () => {
  document.body.addEventListener("keydown", onKey);
  try {
    await listen("feedback:state", (e) => show(e.payload || {}));
  } catch {
    /* 订阅失败仍可由 feedback_state 拉一次 */
  }
  try {
    show(await invoke("feedback_state"));
  } catch {
    close();
  }
});

onBeforeUnmount(() => {
  clearTimeout(timer);
  document.body.removeEventListener("keydown", onKey);
});
</script>

<template>
  <div
    id="card"
    :class="{ fade: closing }"
    :style="{ transitionDuration: fadeMs + 'ms' }"
    @mouseenter="pause"
    @mouseleave="schedule"
  >
    <img v-if="url" id="shot" :src="url" alt="" draggable="false" @load="onLoad" @error="close" />
    <div id="acts">
      <button
        v-for="a in ACTS"
        :key="a.act"
        class="fb-btn"
        type="button"
        :title="a.title"
        :aria-label="a.title"
        @click.stop="act(a.act)"
      >
        <Icon :name="a.icon" :size="20" />
      </button>
    </div>
  </div>
</template>
