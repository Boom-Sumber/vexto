import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import MainApp from "../main/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "main.windowTitle" });

const theme = useTheme();
await theme.init();

createVextoApp(MainApp).mount("#app");
