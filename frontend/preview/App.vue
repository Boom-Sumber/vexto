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
      <button id="pv-crop" class="tb-btn" type="button" :title="$t('preview.crop')"></button>
      <span class="pv-edit-sep"></span>
      <button id="pv-pin" class="tb-btn" type="button" :title="$t('region.pin')"></button>
      <button id="pv-copy" class="tb-btn" type="button" :title="$t('preview.copy')"></button>
      <span class="pv-toolbar-fill"></span>

      <div class="pv-save-group">
        <button id="pv-save" class="btn btn-primary pv-save-main" type="button">
          {{ $t("preview.save") }}
        </button>
        <button
          id="pv-save-caret"
          class="btn btn-primary pv-save-caret"
          type="button"
          :title="$t('preview.saveAs')"
          aria-haspopup="menu"
          aria-expanded="false"
        ></button>
        <div id="pv-save-menu" class="pv-save-menu" role="menu" hidden>
          <button type="button" role="menuitem" data-save="save">{{ $t("preview.save") }}</button>
          <button type="button" role="menuitem" data-save="save_as">{{ $t("preview.saveAs") }}</button>
        </div>
      </div>

      <button id="pv-ocr" class="btn pv-ocr-btn" type="button">
        <span id="pv-ocr-ico" class="pv-btn-ico"></span>
        <span>{{ $t("preview.recognize") }}</span>
      </button>
    </div>

    <div id="pv-msg" class="pv-msg"></div>

    <div id="pv-crop-pop" class="pv-crop-pop" hidden>
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
          <span id="pv-zoom-label" class="pv-zoom-label">100%</span>
          <button id="pv-zoom-in" class="tb-btn" type="button" :title="$t('preview.zoomIn')"></button>
        </div>
      </div>

      <aside class="pv-side">
        <section id="pv-ocr-box" class="pv-card">
          <div class="pv-card-head">
            <span>{{ $t("preview.ocrResult") }}</span>
            <div class="pv-ocr-acts">
              <button id="pv-translate" class="btn btn-ghost ico-btn" type="button" :title="$t('preview.translateResult')"></button>
              <button id="pv-copy-text" class="btn btn-ghost ico-btn" type="button" :title="$t('preview.copyText')"></button>
            </div>
          </div>
          <pre id="pv-ocr-text" :data-empty="$t('preview.ocrEmpty')"></pre>
        </section>

        <section id="pv-trans-box" class="pv-card">
          <div class="pv-card-head">
            <span>{{ $t("preview.transResult") }}</span>
          </div>
          <pre id="pv-trans-text" :data-empty="$t('preview.transEmpty')"></pre>
        </section>
      </aside>
    </main>
  </div>
</template>
