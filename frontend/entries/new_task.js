import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import App from "../new_task/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { t } from "../i18n/index.js";

const theme = useTheme();
await theme.init();

document.title = t("newTask.title");
createVextoApp(App).mount("#app");
