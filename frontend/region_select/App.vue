<script setup>
import { onMounted, ref } from "vue";
import Icon from "../components/Icon.vue";
import { startRegionSelect } from "./controller.js";

const purpose =
  document.body.dataset.purpose === "shot" || document.body.dataset.purpose === "scroll"
    ? document.body.dataset.purpose
    : "ocr";
const root = ref(null);

onMounted(() => {
  startRegionSelect(root.value, purpose);
});
</script>

<template>
  <div ref="root" class="rs-root">
    <template v-if="purpose === 'ocr'">
      <div id="dim"></div>
      <div id="box"></div>

      <div id="card">
        <div id="card-tag" class="card-tag"></div>
        <div id="card-original" class="original"></div>
        <div id="card-text" class="translated"></div>
        <div id="shotPath" class="shot-path" style="display: none"></div>
        <div id="card-acts" class="card-acts">
          <button id="copyBtn" class="btn" type="button">{{ $t("region.copyTranslation") }}</button>
          <button id="closeBtn" class="btn btn-primary" type="button">{{ $t("common.close") }}</button>
        </div>
      </div>
    </template>

    <template v-else>
      <img id="snap" class="snap" alt="" draggable="false" />
      <div id="hit"></div>
      <div id="window-highlight">
        <span class="h h-tl"></span><span class="h h-tc"></span><span class="h h-tr"></span>
        <span class="h h-ml"></span><span class="h h-mr"></span>
        <span class="h h-bl"></span><span class="h h-bc"></span><span class="h h-br"></span>
      </div>
      <div id="size-label"></div>
      <div id="dim"></div>
      <div id="box">
        <img id="result" class="result-img" alt="" draggable="false" />
        <span class="h h-tl"></span><span class="h h-tc"></span><span class="h h-tr"></span>
        <span class="h h-ml"></span><span class="h h-mr"></span>
        <span class="h h-bl"></span><span class="h h-bc"></span><span class="h h-br"></span>
      </div>
      <canvas v-if="purpose === 'shot'" id="edit-canvas" class="edit-canvas" hidden></canvas>

      <div id="magnifier">
        <canvas id="mag-canvas" width="128" height="128"></canvas>
        <div class="mag-info">
          <div class="mag-row">
            <span class="mag-label">{{ $t("region.coord") }}</span>
            <span class="mag-value" id="mag-pos">0, 0</span>
          </div>
          <div class="mag-row">
            <span class="mag-label">{{ $t("region.color") }}</span>
            <span class="mag-value mag-color"><i id="mag-swatch"></i><span id="mag-hex">#000000</span></span>
          </div>
          <div class="mag-hint" id="mag-hint">{{ $t("region.copyColorHint") }}</div>
        </div>
      </div>

      <div id="toolbar">
        <template v-if="purpose === 'shot'">
          <button class="tb-btn" type="button" data-tool="rect" :title="$t('region.rect')"></button>
          <button class="tb-btn" type="button" data-tool="ellipse" :title="$t('region.ellipse')"></button>
          <button class="tb-btn" type="button" data-tool="arrow" :title="$t('region.arrow')"></button>
          <button class="tb-btn" type="button" data-tool="pen" :title="$t('region.pen')"></button>
          <button class="tb-btn" type="button" data-tool="text" :title="$t('region.text')"></button>
          <button class="tb-btn" type="button" data-tool="mosaic" :title="$t('region.mosaic')"></button>
          <span class="tb-sep"></span>
          <button class="tb-btn" type="button" data-act="scroll" :title="$t('region.scrollShot')"></button>
          <span class="tb-sep"></span>
          <button class="tb-btn" type="button" data-act="preview" :title="$t('region.preview')"></button>
          <button class="tb-btn" type="button" data-act="pin" :title="$t('region.pin')"></button>
          <button class="tb-btn" type="button" data-act="copy" :title="$t('region.copyImage')"></button>
          <button class="tb-btn" type="button" data-act="save" :title="$t('region.saveCopy')"></button>
          <button class="tb-btn tb-close" type="button" data-act="close" :title="$t('region.closeNoSave')"></button>
        </template>
        <template v-else>
          <button class="tb-btn" type="button" data-act="save_as" :title="$t('region.saveAs')">
            <Icon name="download" :size="24" />
          </button>
          <button class="tb-btn" type="button" data-act="clipboard" :title="$t('region.copyImage')">
            <Icon name="copy" :size="24" />
          </button>
          <button class="tb-btn" type="button" data-act="path" :title="$t('region.saveAndPath')">
            <Icon name="external" :size="24" />
          </button>
          <button class="tb-btn" type="button" data-act="pin" :title="$t('region.pin')">
            <Icon name="pin" :size="24" />
          </button>
          <button class="tb-btn" type="button" data-act="preview" :title="$t('region.preview')">
            <Icon name="preview" :size="24" />
          </button>
          <span class="tb-sep"></span>
          <button class="tb-btn tb-ok" type="button" data-act="default" :title="$t('region.done')">
            <Icon name="check" :size="24" />
          </button>
          <button class="tb-btn tb-close" type="button" data-act="close" :title="$t('common.close')">
            <Icon name="close" :size="24" />
          </button>
        </template>
      </div>
      <div v-if="purpose === 'shot'" id="scroll-hint">{{ $t("region.scrollHint") }}</div>
      <div id="toast" role="status" aria-live="polite"></div>

      <div id="card">
        <div id="card-tag" class="card-tag"></div>
        <div id="card-original" class="original"></div>
        <div id="card-text" class="translated"></div>
        <div id="shotPath" class="shot-path" style="display: none"></div>
        <div id="card-acts" class="card-acts">
          <button id="pinBtn" class="btn" type="button" style="display: none">{{ $t("region.pin") }}</button>
          <button id="previewBtn" class="btn" type="button" style="display: none">{{ $t("region.preview") }}</button>
          <button id="copyBtn" class="btn" type="button">{{ $t("region.copyTranslation") }}</button>
          <button id="closeBtn" class="btn btn-primary" type="button">{{ $t("common.close") }}</button>
        </div>
      </div>
    </template>
  </div>
</template>
