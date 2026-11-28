import "../shared/contextmenu.js";
import "../shared/theme.js";
import "../shared/icons.js";
import "../shared/annot_editor.js";
import { createVextoApp } from "../app.js";
import PreviewApp from "../preview/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "preview.title", reloadOnChange: true });

const theme = useTheme();
await theme.init();

createVextoApp(PreviewApp).mount("#app");
