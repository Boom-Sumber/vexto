import "../shared/contextmenu.js";
import "../shared/theme.js";
import "../shared/icons.js";
import { createVextoApp } from "../app.js";
import RegionApp from "../region_select/App.vue";

document.body.dataset.purpose = "ocr";
createVextoApp(RegionApp).mount("#app");
