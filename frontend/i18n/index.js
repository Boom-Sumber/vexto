import { createI18n } from "vue-i18n";
import zhCN from "./locales/zh-CN.json";
import en from "./locales/en.json";
import ja from "./locales/ja.json";
import ko from "./locales/ko.json";
import es from "./locales/es.json";

export const SUPPORTED_LOCALES = ["zh-CN", "en", "ja", "ko", "es"];

/** 空串视为「未翻译」，装载前剔除，避免挡住 fallbackLocale。 */
function compact(node) {
  if (node == null || typeof node !== "object") return node;
  const out = {};
  for (const [key, value] of Object.entries(node)) {
    if (value && typeof value === "object") {
      out[key] = compact(value);
    } else if (typeof value === "string" && value !== "") {
      out[key] = value;
    }
  }
  return out;
}

const messages = {
  "zh-CN": compact(zhCN),
  en: compact(en),
  ja: compact(ja),
  ko: compact(ko),
  es: compact(es),
};

/** vue-i18n 实例；legacy=false 走 Composition API，模板仍保留 $t。 */
export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  locale: "zh-CN",
  fallbackLocale: {
    default: ["en", "zh-CN"],
    "zh-CN": ["zh-CN"],
  },
  messages,
  missingWarn: import.meta.env.DEV,
  fallbackWarn: import.meta.env.DEV,
});

export const locale = i18n.global.locale;

/** 兼容旧调用点：脚本 / controller 里的 t(key, params)。 */
export function t(key, params) {
  return params === undefined ? i18n.global.t(key) : i18n.global.t(key, params);
}

export function setLocale(next) {
  const code = SUPPORTED_LOCALES.includes(next) ? next : "en";
  i18n.global.locale.value = code;
  document.documentElement.lang = code;
}

/** 兼容旧调用点：组件内 const { t } = useI18n()。 */
export function useI18n() {
  return { t, locale, setLocale };
}

/** Vue 插件安装入口；app.js 的 createVextoApp 统一调用。 */
export function installI18n(app) {
  app.use(i18n);
}

export function normalizeLocale(raw) {
  const value = String(raw || "").toLowerCase();
  if (value.startsWith("zh")) return "zh-CN";
  if (value.startsWith("ja")) return "ja";
  if (value.startsWith("ko")) return "ko";
  if (value.startsWith("es")) return "es";
  if (value.startsWith("en")) return "en";
  return "en";
}

/**
 * 页面挂载前初始化语言：
 * - 优先问 Rust 要已解析的 UI locale（system 已按 Windows 语言映射）；
 * - 不在 Tauri 里（probe / vite dev）时退回 navigator.language；
 * - 监听 i18n:changed，Vue 页面自动重渲染，controller 页面按需 reload。
 */
export async function initI18n({ titleKey, reloadOnChange = false } = {}) {
  const invoke = window.__TAURI__?.core?.invoke;
  let code;
  if (invoke) {
    try {
      code = normalizeLocale(await invoke("get_ui_locale"));
    } catch (e) {
      if (import.meta.env.DEV) console.warn("[i18n] get_ui_locale failed", e);
      code = normalizeLocale(navigator.language);
    }
  } else {
    code = normalizeLocale(navigator.language);
  }
  setLocale(code);
  if (titleKey) document.title = t(titleKey);

  const listen = window.__TAURI__?.event?.listen;
  if (!listen) return;
  try {
    await listen("i18n:changed", (event) => {
      setLocale(normalizeLocale(event?.payload?.locale));
      if (titleKey) document.title = t(titleKey);
      if (reloadOnChange) location.reload();
    });
  } catch (e) {
    if (import.meta.env.DEV) console.warn("[i18n] listen failed", e);
  }
}
