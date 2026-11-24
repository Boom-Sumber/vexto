<script setup>
import { nextTick, onMounted, ref } from "vue";
import TitleBar from "../components/TitleBar.vue";
import { startPreview } from "./controller.js";

const root = ref(null);
const tb = ref(null);

onMounted(async () => {
  startPreview(root.value, {
    onReady: () => nextTick(() => tb.value?.reveal()),
  });
});
</script>

<template>
  <div ref="root" class="win">
    <TitleBar ref="tb" :title="$t('preview.title')" />

    <div class="pv-head">
      <span id="pv-info" class="pv-info">{{ $t("preview.reading") }}</span>
      <span class="pv-hint">{{ $t("preview.cropHint") }}</span>
    </div>

    <div id="pv-edit-bar" class="pv-edit-bar">
      <button id="pv-tool-move" class="tb-btn pv-tool on" type="button" data-tool="" :title="$t('preview.selectTool')"></button>
      <span class="pv-edit-sep"></span>
      <button class="tb-btn pv-tool" type="button" data-tool="rect" :title="$t('region.rect')"></button>
      <button class="tb-btn pv-tool" type="button" data-tool="ellipse" :title="$t('region.ellipse')"></button>
      <button class="tb-btn pv-tool" type="button" data-tool="arrow" :title="$t('region.arrow')"></button>
      <button class="tb-btn pv-tool" type="button" data-tool="pen" :title="$t('region.pen')"></button>
      <button class="tb-btn pv-tool" type="button" data-tool="text" :title="$t('region.text')"></button>
      <button class="tb-btn pv-tool" type="button" data-tool="mosaic" :title="$t('region.mosaic')"></button>
      <span class="pv-edit-sep"></span>
      <span id="pv-edit-hint" class="pv-edit-hint">{{ $t("preview.editHint") }}</span>
    </div>

    <main class="pv-main">
      <div class="pv-viewport-wrap">
        <div class="pv-viewport">
          <div id="pv-canvas" class="pv-canvas">
            <img id="pv-img" alt="" draggable="false" />
            <canvas id="pv-annot" class="pv-annot"></canvas>
            <div id="pv-band-top" class="pv-band top"></div>
            <div id="pv-band-bottom" class="pv-band bottom"></div>
            <div id="pv-band-left" class="pv-band left"></div>
            <div id="pv-band-right" class="pv-band right"></div>
          </div>
        </div>
        <div class="pv-zoom">
          <button id="pv-zoom-out" class="tb-btn" type="button" :title="$t('preview.zoomOut')"></button>
          <button id="pv-zoom-in" class="tb-btn" type="button" :title="$t('preview.zoomIn')"></button>
        </div>
      </div>

      <aside class="pv-side">
        <div class="pv-field">
          <div class="pv-label"><span>{{ $t("preview.cropTop") }}</span><span id="pv-head-v">0 px</span></div>
          <input id="pv-head" type="range" min="0" max="0" value="0" step="1" />
        </div>
        <div class="pv-field">
          <div class="pv-label"><span>{{ $t("preview.cropBottom") }}</span><span id="pv-tail-v">0 px</span></div>
          <input id="pv-tail" type="range" min="0" max="0" value="0" step="1" />
        </div>
        <div class="pv-field">
          <div class="pv-label"><span>{{ $t("preview.cropLeft") }}</span><span id="pv-left-v">0 px</span></div>
          <input id="pv-left" type="range" min="0" max="0" value="0" step="1" />
        </div>
        <div class="pv-field">
          <div class="pv-label"><span>{{ $t("preview.cropRight") }}</span><span id="pv-right-v">0 px</span></div>
          <input id="pv-right" type="range" min="0" max="0" value="0" step="1" />
        </div>
        <p class="pv-hint">{{ $t("preview.cropHint2") }}</p>

        <div class="pv-field">
          <div class="pv-label"><span>{{ $t("preview.saveDir") }}</span></div>
          <div class="pv-path-row">
            <input id="pv-dir" type="text" readonly />
            <button id="pv-open-dir" class="btn icon-btn" type="button" :title="$t('preview.openDir')"></button>
          </div>
        </div>

        <div class="pv-actions">
          <button id="pv-save" class="btn btn-primary" type="button">{{ $t("preview.save") }}</button>
          <button id="pv-copy" class="btn" type="button">{{ $t("preview.copy") }}</button>
          <button id="pv-pin" class="btn" type="button">{{ $t("region.pin") }}</button>
          <button id="pv-ocr" class="btn" type="button">{{ $t("preview.recognize") }}</button>
        </div>

        <div id="pv-msg" class="pv-msg"></div>

        <div id="pv-ocr-box" class="pv-ocr" hidden>
          <div class="pv-ocr-head">
            <span>{{ $t("preview.ocrResult") }}</span>
            <div class="pv-ocr-acts">
              <button id="pv-translate" class="btn btn-ghost ico-btn" type="button" :title="$t('preview.translateResult')"></button>
              <button id="pv-copy-text" class="btn btn-ghost ico-btn" type="button" :title="$t('preview.copyText')"></button>
            </div>
          </div>
          <pre id="pv-ocr-text"></pre>
        </div>
      </aside>
    </main>
  </div>
</template>
