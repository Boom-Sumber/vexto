import "../shared/contextmenu.js";
import "../shared/theme.js";
import "../shared/icons.js";
import "../shared/annot_editor.js";
import { createVextoApp } from "../app.js";
import PreviewApp from "../preview/App.vue";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "preview.title", reloadOnChange: true });

createVextoApp(PreviewApp).mount("#app");
