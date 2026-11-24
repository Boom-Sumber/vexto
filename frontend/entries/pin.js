import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import PinApp from "../pin/App.vue";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "pin.windowTitle", reloadOnChange: true });

createVextoApp(PinApp).mount("#app");
