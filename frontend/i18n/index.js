import { ref } from "vue";
import zhCN from "./locales/zh-CN.json";
import en from "./locales/en.json";

const catalogs = { "zh-CN": zhCN, en };
export const locale = ref("zh-CN");

function lookup(catalog, key) {
  return key.split(".").reduce((node, part) => (node == null ? node : node[part]), catalog);
}

function format(text, params) {
  return text.replace(/\{(\w+)\}/g, (_, name) =>
    params && name in params ? String(params[name]) : `{${name}}`,
  );
}

/** 轻量文案函数；签名与 vue-i18n 的 $t(key, params) 子集一致。 */
export function t(key, params) {
  const primary = lookup(catalogs[locale.value], key);
  const fallback = lookup(catalogs["zh-CN"], key);
  const text = typeof primary === "string" && primary !== "" ? primary : fallback;
  if (typeof text !== "string") {
    if (import.meta.env.DEV) console.warn(`[i18n] missing key: ${key}`);
    return key;
  }
  return params ? format(text, params) : text;
}

export function setLocale(next) {
  if (catalogs[next]) locale.value = next;
}

export function useI18n() {
  return { t, locale, setLocale };
}

/** Vue 模板里的 $t；未来换 vue-i18n 时只替换这里。 */
export function installI18n(app) {
  app.config.globalProperties.$t = t;
}
