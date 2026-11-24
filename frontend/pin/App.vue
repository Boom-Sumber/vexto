<script setup>
import { onMounted, ref } from "vue";
import Icon from "../components/Icon.vue";
import { startPin } from "./controller.js";

const root = ref(null);
onMounted(() => startPin(root.value));
</script>

<template>
  <div ref="root" class="pin-root">
    <div id="stage" data-tauri-drag-region>
      <img id="img" alt="" draggable="false" data-tauri-drag-region />
    </div>

    <div id="frame" class="hidden">
      <span class="ph nw"></span><span class="ph n"></span><span class="ph ne"></span>
      <span class="ph e"></span><span class="ph se"></span><span class="ph s"></span>
      <span class="ph sw"></span><span class="ph w"></span>
    </div>

    <div id="bar">
      <div id="nav">
        <button id="prev" :title="$t('pin.prev')">
          <Icon name="arrow-left" :size="15" />
        </button>
        <span id="idx">1/1</span>
        <button id="next" :title="$t('pin.next')">
          <Icon name="arrow-right" :size="15" />
        </button>
        <span class="sep"></span>
      </div>
      <button id="preview" :title="$t('region.preview')">
        <Icon name="preview" :size="15" />
      </button>
      <button id="copy" :title="$t('pin.copyImage')">
        <Icon name="copy" :size="15" />
      </button>
      <span id="zoom">100%</span>
      <button id="reset" :title="$t('pin.resetZoom')">
        <Icon name="expand" :size="15" />
      </button>
      <button id="close" :title="$t('common.close')">
        <Icon name="close" :size="15" />
      </button>
    </div>
    <div id="msg" role="status" aria-live="polite"></div>
  </div>
</template>
