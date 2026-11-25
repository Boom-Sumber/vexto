<script setup>
import { computed, ref, watch } from "vue";
import Icon from "../components/Icon.vue";
import DropdownSelect from "../components/DropdownSelect.vue";
import SegmentedControl from "../components/SegmentedControl.vue";
import { preferSegmented, toItems } from "../components/choiceOptions.js";
import HelpIcon from "./HelpIcon.vue";
import ListField from "./ListField.vue";
import ToolListField from "./ToolListField.vue";
import HotkeyInput from "./HotkeyInput.vue";
import AboutPage from "./AboutPage.vue";
import { vNumField } from "../directives/numField.js";
import { t } from "../i18n/index.js";
import { engineChoices, errText, showIfMatch } from "./utils.js";

const props = defineProps({
  field: { type: Object, required: true },
  settings: { type: Object, required: true },
});

const visible = computed(() => showIfMatch(props.field.show_if, props.settings.val));
const currentValue = computed(() => props.settings.val(props.field.key));
const isTextLike = computed(() =>
  ["text", "password", "dir", "template"].includes(props.field.type),
);

const draft = ref("");
const editing = ref(false);
const preview = ref("");
const previewBad = ref(false);
const previewing = ref(false);

function syncDraft() {
  const v = currentValue.value;
  draft.value = v == null ? "" : String(v);
}

watch(
  currentValue,
  () => {
    if (!editing.value) syncDraft();
  },
);
watch(
  () => props.field.key,
  () => {
    editing.value = false;
    syncDraft();
  },
  { immediate: true },
);

function onDraftInput(e) {
  draft.value = e.target.value;
}

function onTemplateInput(e) {
  draft.value = e.target.value;
  preview.value = "";
}

function commitText() {
  editing.value = false;
  props.settings.commit(props.field.key, draft.value.trim());
}

function commitNumber() {
  let n = Number(draft.value);
  if (!Number.isFinite(n)) {
    syncDraft();
    return;
  }
  if (typeof props.field.min === "number" && n < props.field.min) n = props.field.min;
  if (typeof props.field.max === "number" && n > props.field.max) n = props.field.max;
  draft.value = String(n);
  props.settings.commit(props.field.key, n);
}

function applyToken(token) {
  const cur = draft.value;
  const m = /\$[^$]*\$/.exec(cur);
  let next;
  if (m) {
    next = cur.slice(0, m.index) + "$" + token + "$" + cur.slice(m.index + m[0].length);
  } else {
    const ext = /\.png$/i.exec(cur);
    next = ext
      ? cur.slice(0, ext.index) + "$" + token + "$" + cur.slice(ext.index)
      : cur + "$" + token + "$";
  }
  draft.value = next;
  preview.value = "";
  props.settings.commit(props.field.key, next);
}

function applyPreset(name, value) {
  if (props.field.type === "template") {
    applyToken(value);
    return;
  }
  draft.value = String(value);
  editing.value = false;
  props.settings.commit(props.field.key, String(value));
}

async function runPreview() {
  if (!props.field.preview_command || previewing.value) return;
  previewing.value = true;
  try {
    const name = await window.__TAURI__.core.invoke(props.field.preview_command, {
      rule: draft.value,
    });
    preview.value = t("settings.previewSample", { name });
    previewBad.value = false;
  } catch (e) {
    preview.value = errText(e);
    previewBad.value = true;
  } finally {
    previewing.value = false;
  }
}

function toggleSwitch() {
  props.settings.commit(props.field.key, !props.settings.val(props.field.key));
}

function toggleMeta() {
  if (props.field.type === "switch") toggleSwitch();
}

const selectOptions = computed(() => {
  if (props.field.dynamic) {
    const opts = engineChoices(props.settings.val);
    return opts.length ? opts : [["", t("settings.noEngines")]];
  }
  const opts = (props.field.options || []).map(([v, txt]) => [String(v), txt]);
  const cur = currentValue.value;
  if (cur != null && String(cur) !== "" && !opts.some(([v]) => v === String(cur))) {
    opts.push([String(cur), String(cur)]);
  }
  return opts;
});
const noOptions = computed(
  () => !!props.field.dynamic && engineChoices(props.settings.val).length === 0,
);

// 2–3 个短选项用分段滑块，其余用自绘下拉（原生 select 的弹层由系统画，和主题不搭）
const segmented = computed(() => preferSegmented(toItems(selectOptions.value)));

function commitValue(value) {
  props.settings.commit(props.field.key, value, {
    autoApply: !!props.field.auto_apply,
  });
}

const colorActive = computed(
  () => String(currentValue.value || "") || props.field.presets?.[0]?.[0] || "",
);
const customColorValue = computed(() =>
  /^#[0-9a-fA-F]{6}$/.test(String(currentValue.value || ""))
    ? currentValue.value
    : "#11a780",
);

function pickColor(value) {
  props.settings.commit(props.field.key, value === colorActive.value ? "" : value, {
    autoApply: !!props.field.auto_apply,
  });
}

function pickCustom(e) {
  props.settings.commit(props.field.key, e.target.value, {
    autoApply: !!props.field.auto_apply,
  });
}

function commitHotkey(value) {
  props.settings.commit(props.field.key, value);
}

// 恢复出厂设置必须二次确认；分组恢复只重置热键，直接执行。
const DANGER_ACTIONS = new Set(["reset_all_config"]);
const RESET_ACTIONS = new Set([
  "reset_hotkeys_translate",
  "reset_hotkeys_screenshot",
  "reset_hotkeys_download",
]);

async function runAction(key, btn) {
  if (DANGER_ACTIONS.has(key)) {
    const ok = await props.settings.confirmDanger(key);
    if (!ok) return;
  }
  if (btn) btn.disabled = true;
  try {
    const res = await window.__TAURI__.core.invoke(key);
    props.settings.showBanner("");
    const bad = res && res.ok === false;
    props.settings.showToast(
      (res && res.message) || (bad ? t("settings.toggleFailed") : t("settings.saved")),
      bad ? "bad" : "",
    );
    // 全量恢复后进程马上重启，不需要再读配置。
    if (DANGER_ACTIONS.has(key) && !bad) return;
    await props.settings.refreshAfterSave();
    if (RESET_ACTIONS.has(key) && !bad) await props.settings.refreshAll();
  } catch (e) {
    props.settings.showToast(errText(e), "bad");
  } finally {
    if (btn) btn.disabled = false;
  }
}

</script>

<template>
  <div v-if="field.type === 'note'" class="field note" v-show="visible">{{ field.text }}</div>

  <div v-else-if="field.type === 'group'" class="group-title">{{ field.title }}</div>

  <ListField
    v-else-if="field.type === 'list'"
    v-show="visible"
    :field="field"
    :settings="settings"
  />

  <div v-else-if="field.type === 'tool_list'" class="field tool-list" v-show="visible">
    <ToolListField :settings="settings" />
  </div>

  <div v-else-if="field.type === 'about'" class="field about" v-show="visible">
    <AboutPage :field="field" :settings="settings" />
  </div>

  <div v-else-if="field.type === 'button'" class="field action" v-show="visible">
    <div class="btn-row">
      <button class="btn" type="button" @click="runAction(field.key, $event.currentTarget)">
        {{ field.label || field.key }}
      </button>
      <HelpIcon v-if="field.help" :text="field.help" />
    </div>
  </div>

  <div v-else-if="field.type === 'button_row'" class="field action" v-show="visible">
    <div class="btn-row">
      <button
        v-for="[key, label] in field.actions || []"
        :key="key"
        class="btn"
        :class="{ danger: DANGER_ACTIONS.has(key), subtle: RESET_ACTIONS.has(key) }"
        type="button"
        @click="runAction(key, $event.currentTarget)"
      >
        {{ label }}
      </button>
    </div>
  </div>

  <div
    v-else
    class="field"
    :class="{ wide: field.wide, switch: field.type === 'switch' }"
    v-show="visible"
  >
    <div class="setting-row">
      <div class="meta" @click="toggleMeta">
        <div class="t">
          {{ field.label || field.key }}
          <HelpIcon v-if="field.help && field.help_tip" :text="field.help" />
        </div>
        <div v-if="field.help && !field.help_tip" class="d">{{ field.help }}</div>
      </div>

      <div class="ctrl">
        <template v-if="isTextLike">
          <input
            class="input"
            :type="field.type === 'password' ? 'password' : 'text'"
            :value="draft"
            :placeholder="field.placeholder || (field.type === 'dir' ? $t('settings.dirPlaceholder') : '')"
            spellcheck="false"
            autocomplete="off"
            @focus="editing = true"
            @input="field.type === 'template' ? onTemplateInput($event) : onDraftInput($event)"
            @blur="commitText"
            @keydown.enter.prevent="commitText"
          />
          <div v-if="field.presets && field.presets.length" class="presets">
            <button
              v-for="[name, value] in field.presets"
              :key="name"
              class="preset-btn"
              type="button"
              :title="field.type === 'template' ? '$' + value + '$' : value"
              @click="applyPreset(name, value)"
            >
              {{ name }}
            </button>
          </div>
          <div v-if="field.type === 'template' && field.preview_command" class="tpl-preview-row">
            <button class="btn" type="button" :disabled="previewing" @click="runPreview">
              {{ $t("settings.preview") }}
            </button>
            <div v-if="preview" class="tpl-preview" :class="{ bad: previewBad }">{{ preview }}</div>
          </div>
        </template>

        <template v-else-if="field.type === 'number'">
          <div class="num-row" :class="{ 'has-unit': field.unit }">
            <input
              v-num-field
              class="input"
              type="number"
              :value="draft"
              :min="field.min"
              :max="field.max"
              @focus="editing = true"
              @input="onDraftInput"
              @change="commitNumber"
              @blur="commitNumber"
            />
            <span v-if="field.unit" class="num-unit">{{ field.unit }}</span>
          </div>
        </template>

        <label v-else-if="field.type === 'switch'" class="switch-pill">
          <input
            type="checkbox"
            :checked="!!currentValue"
            @change="settings.commit(field.key, $event.target.checked)"
          />
          <span class="track"></span>
          <span class="thumb"></span>
        </label>

        <SegmentedControl
          v-else-if="field.type === 'select' && segmented"
          :model-value="String(currentValue ?? '')"
          :options="selectOptions"
          :disabled="noOptions"
          @update:model-value="commitValue"
        />

        <DropdownSelect
          v-else-if="field.type === 'select'"
          :model-value="String(currentValue ?? '')"
          :options="selectOptions"
          :disabled="noOptions"
          @update:model-value="commitValue"
        />

        <div v-else-if="field.type === 'color'" class="swatches">
          <button
            v-for="[value, name] in field.presets || []"
            :key="value"
            class="swatch"
            :class="{ on: value === colorActive }"
            :style="{ background: value }"
            :title="name"
            :aria-label="name"
            type="button"
            @click="pickColor(value)"
          ></button>
          <label class="swatch custom" :title="$t('settings.customColor')">
            <input
              type="color"
              :value="customColorValue"
              @input="pickCustom"
            />
          </label>
        </div>

        <HotkeyInput
          v-else-if="field.type === 'hotkey'"
          :model-value="currentValue"
          :label="field.label || field.key"
          @update:model-value="commitHotkey"
        />

        <div v-else class="note">
          {{ $t("settings.unknownField", { type: field.type }) }}
        </div>
      </div>
    </div>
  </div>
</template>
