<script setup>
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { t } from "../i18n/index.js";

const props = defineProps({
  modelValue: { type: [Object, String], default: null },
  label: { type: String, default: "" },
});

const emit = defineEmits(["update:modelValue"]);

const capturing = ref(false);
const liveMods = ref([]);
const liveKey = ref("");
const hint = ref("");

const MOD_ORDER = ["Ctrl", "Alt", "Shift"];
const MOD_KEYS = { Control: "Ctrl", Alt: "Alt", Shift: "Shift" };

// event.code → 配置里的规范主键；与后端 hotkey.rs 的白名单保持一致。
const CODE_MAP = {
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Insert: "Insert",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Backspace: "Backspace",
  Space: "Space",
  Tab: "Tab",
  Enter: "Enter",
  Escape: "Esc",
  PrintScreen: "PrintScreen",
};

const KEY_LABEL = {
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Insert: "Ins",
  Delete: "Del",
  PageUp: "PgUp",
  PageDown: "PgDn",
  Esc: "Esc",
};

function normalize(value) {
  if (!value) return { modifiers: [], key: "", match: "exact" };
  if (typeof value === "string") {
    const parts = value
      .split("+")
      .map((part) => part.trim())
      .filter(Boolean);
    if (!parts.length) return { modifiers: [], key: "", match: "exact" };
    const key = parts.pop();
    const modifiers = MOD_ORDER.filter((mod) =>
      parts.some((part) => part.toLowerCase() === mod.toLowerCase()),
    );
    return { modifiers, key: canonicalKey(key), match: "exact" };
  }
  const modifiers = MOD_ORDER.filter((mod) =>
    Array.isArray(value.modifiers)
      ? value.modifiers.some((item) => String(item).toLowerCase() === mod.toLowerCase())
      : false,
  );
  return { modifiers, key: canonicalKey(value.key), match: "exact" };
}

function canonicalKey(raw) {
  const text = String(raw ?? "").trim();
  if (!text) return "";
  const upper = text.toUpperCase();
  if (upper.length === 1 && /[A-Z0-9]/.test(upper)) return upper;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(upper)) return upper;
  const named = {
    UP: "Up",
    DOWN: "Down",
    LEFT: "Left",
    RIGHT: "Right",
    INSERT: "Insert",
    INS: "Insert",
    DELETE: "Delete",
    DEL: "Delete",
    HOME: "Home",
    END: "End",
    PAGEUP: "PageUp",
    PGUP: "PageUp",
    PAGEDOWN: "PageDown",
    PGDN: "PageDown",
    BACKSPACE: "Backspace",
    SPACE: "Space",
    TAB: "Tab",
    ENTER: "Enter",
    RETURN: "Enter",
    ESC: "Esc",
    ESCAPE: "Esc",
    PRINTSCREEN: "PrintScreen",
  };
  return named[upper] || text;
}

function codeToKey(code) {
  if (!code) return "";
  if (CODE_MAP[code]) return CODE_MAP[code];
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return "";
}

function keyLabel(key) {
  return KEY_LABEL[key] || key;
}

const value = computed(() => normalize(props.modelValue));
const displayMods = computed(() =>
  capturing.value ? liveMods.value : value.value.modifiers,
);
const displayKey = computed(() =>
  capturing.value ? liveKey.value : value.value.key,
);
const isEmpty = computed(() => !value.value.key);
const showPrompt = computed(
  () => capturing.value && !displayMods.value.length && !displayKey.value,
);
const showPlaceholder = computed(() =>
  capturing.value ? showPrompt.value : isEmpty.value,
);

watch(
  () => props.modelValue,
  () => {
    if (!capturing.value) hint.value = "";
  },
);

function begin() {
  if (capturing.value) return;
  capturing.value = true;
  liveMods.value = [];
  liveKey.value = "";
  hint.value = "";
  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("keyup", onKeyUp, true);
}

function stop() {
  window.removeEventListener("keydown", onKeyDown, true);
  window.removeEventListener("keyup", onKeyUp, true);
  capturing.value = false;
  liveMods.value = [];
  liveKey.value = "";
  hint.value = "";
}

function cancel() {
  stop();
}

function modsFromEvent(event) {
  return MOD_ORDER.filter((name) =>
    name === "Ctrl"
      ? event.ctrlKey
      : name === "Alt"
        ? event.altKey
        : event.shiftKey,
  );
}

function onKeyUp(event) {
  if (!capturing.value) return;
  liveMods.value = modsFromEvent(event);
}

function onKeyDown(event) {
  if (!capturing.value) return;
  event.preventDefault();
  event.stopPropagation();

  if (MOD_KEYS[event.key]) {
    liveMods.value = modsFromEvent(event);
    hint.value = "";
    return;
  }

  // 单独 Esc 取消；带 Ctrl/Alt 时允许把 Esc 录成主键。
  if (event.code === "Escape" && !event.ctrlKey && !event.altKey) {
    cancel();
    return;
  }

  const key = codeToKey(event.code);
  if (!key || key === "F12") {
    hint.value = t("settings.hotkeyUnsupported");
    return;
  }
  const mods = modsFromEvent(event);
  if (!mods.some((name) => name === "Ctrl" || name === "Alt")) {
    liveMods.value = mods;
    liveKey.value = key;
    hint.value = t("settings.hotkeyNeedModifier");
    return;
  }

  liveMods.value = mods;
  liveKey.value = key;
  emit("update:modelValue", {
    modifiers: mods,
    key,
    match: "exact",
  });
  stop();
}
function clear(event) {
  event.preventDefault();
  event.stopPropagation();
  if (capturing.value) stop();
  emit("update:modelValue", { modifiers: [], key: "", match: "exact" });
}

function onBlur() {
  if (capturing.value) cancel();
}

onBeforeUnmount(() => {
  if (capturing.value) stop();
});
</script>

<template>
  <div class="hotkey-wrap">
    <div
      class="hotkey-input"
      :class="{ capturing, empty: isEmpty && !capturing }"
      role="button"
      tabindex="0"
      :aria-label="label"
      @click="begin"
      @focus="begin"
      @blur="onBlur"
    >
      <span v-if="showPlaceholder" class="hk-prompt">{{ $t("settings.hotkeyEmpty") }}</span>
      <span v-else class="hk-combo">
        <template v-for="(mod, index) in displayMods" :key="mod">
          <span v-if="index" class="hk-plus">+</span>
          <span class="hk-seg">{{ mod }}</span>
        </template>
        <template v-if="displayKey">
          <span v-if="displayMods.length" class="hk-plus">+</span>
          <span class="hk-seg hk-key">{{ keyLabel(displayKey) }}</span>
        </template>
        <span v-else-if="displayMods.length" class="hk-plus hk-more">+ …</span>
      </span>

      <button
        v-if="!capturing && !isEmpty"
        class="hk-clear"
        type="button"
        :title="$t('settings.hotkeyClear')"
        :aria-label="$t('settings.hotkeyClear')"
        @mousedown.prevent.stop
        @click="clear"
      >
        ×
      </button>
    </div>
    <div v-if="hint" class="hk-hint">{{ hint }}</div>
  </div>
</template>
