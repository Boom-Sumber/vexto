<script setup>
import { onBeforeUnmount, onMounted } from "vue";
import Icon from "./Icon.vue";
import { useTheme } from "../composables/useTheme.js";

const props = defineProps({
  title: { type: String, default: "" },
  resize: { type: Boolean, default: true },
  maximize: { type: Boolean, default: true },
});

const emit = defineEmits(["ready"]);
const { resolved, toggle } = useTheme();

const invoke = window.__TAURI__?.core?.invoke ?? (async () => ({}));
const getWindow =
  window.__TAURI__?.window?.getCurrentWindow ??
  (() => ({ startResizeDragging: async () => {} }));

const DIRS = [
  ["n", "North"],
  ["s", "South"],
  ["e", "East"],
  ["w", "West"],
  ["ne", "NorthEast"],
  ["nw", "NorthWest"],
  ["se", "SouthEast"],
  ["sw", "SouthWest"],
];

let revealed = false;
let fallback = 0;

function reveal() {
  if (revealed) return;
  revealed = true;
  clearTimeout(fallback);
  invoke("window_ready").catch(() => {});
  emit("ready");
}

defineExpose({ reveal });

async function action(act) {
  try {
    await invoke("window_action", { action: act });
  } catch (e) {
    console.error("window_action", act, e);
  }
}

async function onClose() {
  if (typeof window.__beforeClose === "function") {
    try {
      if ((await window.__beforeClose()) === false) return;
    } catch (e) {
      console.error("beforeClose", e);
    }
  }
  action("close");
}

function onDblClick(ev) {
  if (!props.maximize || ev.target.closest(".tb-actions")) return;
  action("max");
}

function startResize(dir) {
  getWindow().startResizeDragging(dir).catch(() => {});
}

function syncFocus(on) {
  document.body.classList.toggle("win-blur", !on);
}

const onFocus = () => syncFocus(true);
const onBlur = () => syncFocus(false);

onMounted(() => {
  fallback = setTimeout(reveal, 1500);
  window.addEventListener("focus", onFocus);
  window.addEventListener("blur", onBlur);
  syncFocus(document.hasFocus());
});

onBeforeUnmount(() => {
  clearTimeout(fallback);
  window.removeEventListener("focus", onFocus);
  window.removeEventListener("blur", onBlur);
});
</script>

<template>
  <header class="tb" data-tauri-drag-region @dblclick="onDblClick">
    <div class="tb-lead" data-tauri-drag-region>
      <span class="tb-logo logo-badge" aria-hidden="true" data-tauri-drag-region />
      <span class="tb-title" data-tauri-drag-region>{{ title }}</span>
    </div>
    <div class="tb-actions">
      <button
        type="button"
        class="tb-btn tb-theme"
        :title="$t('common.toggleTheme')"
        :aria-label="$t('common.toggleTheme')"
        @click="toggle"
      >
        <Icon :name="resolved === 'dark' ? 'sun' : 'moon'" :size="14" />
      </button>
      <button type="button" class="tb-btn" data-win="min" :aria-label="$t('common.minimize')" @click="action('min')">
        <Icon name="minimize" :size="14" />
      </button>
      <button
        v-if="maximize"
        type="button"
        class="tb-btn"
        data-win="max"
        :aria-label="$t('common.maximize')"
        @click="action('max')"
      >
        <Icon name="maximize" :size="14" />
      </button>
      <button type="button" class="tb-btn close" data-win="close" :aria-label="$t('common.close')" @click="onClose">
        <Icon name="close" :size="14" />
      </button>
    </div>
  </header>
  <div
    v-for="[cls, dir] in (resize ? DIRS : [])"
    :key="cls"
    :class="['rz', 'rz-' + cls]"
    @pointerdown.prevent="startResize(dir)"
  />
</template>
