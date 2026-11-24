import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import MainApp from "../main/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { t } from "../i18n/index.js";

const theme = useTheme();
await theme.init();

document.title = t("main.windowTitle");
createVextoApp(MainApp).mount("#app");
