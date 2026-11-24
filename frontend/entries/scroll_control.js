import "../shared/contextmenu.js";
import "../shared/theme.js";
import { createVextoApp } from "../app.js";
import ScrollControlApp from "../scroll_control/App.vue";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "scroll.title", reloadOnChange: true });

createVextoApp(ScrollControlApp).mount("#app");
