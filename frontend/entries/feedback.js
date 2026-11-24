import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import App from "../feedback/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { t } from "../i18n/index.js";

const theme = useTheme();
await theme.init();

document.title = t("feedback.windowTitle");
createVextoApp(App).mount("#app");
