import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const frontendRoot = path.join(root, "frontend");
const rustRoot = path.join(root, "src-tauri", "src");
const locales = ["zh-CN", "en", "ja", "ko", "es"];

function flatten(node, prefix = "", out = new Map()) {
  for (const [key, value] of Object.entries(node)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object") flatten(value, full, out);
    else out.set(full, value);
  }
  return out;
}

function placeholders(text, style) {
  const re = style === "rust" ? /%\{(\w+)\}/g : /\{(\w+)\}/g;
  return [...String(text).matchAll(re)].map((m) => m[1]).sort().join(",");
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name === "ui") continue;
      walk(full, out);
    } else if (/\.(vue|js|mjs|rs)$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

const errors = [];
const warnings = [];

function checkCatalogSet({ label, dir, style, scanDir, scanRe, extraValues }) {
  const catalogs = new Map();
  for (const locale of locales) {
    const file = path.join(dir, `${locale}.json`);
    if (!fs.existsSync(file)) {
      errors.push(`${label}: missing ${path.relative(root, file)}`);
      continue;
    }
    const flat = flatten(readJson(file));
    flat.delete("_version");
    catalogs.set(locale, flat);
  }
  const base = catalogs.get("zh-CN");
  if (!base) return { catalogs, unused: new Set() };

  for (const [locale, flat] of catalogs) {
    if (locale === "zh-CN") continue;
    for (const key of base.keys()) {
      if (!flat.has(key)) errors.push(`${label}/${locale}: missing key ${key}`);
    }
    for (const key of flat.keys()) {
      if (!base.has(key)) errors.push(`${label}/${locale}: extra key ${key}`);
    }
    for (const [key, value] of flat) {
      if (typeof value !== "string" || value === "") {
        errors.push(`${label}/${locale}: empty value ${key}`);
        continue;
      }
      if (!base.has(key)) continue;
      const want = placeholders(base.get(key), style);
      const got = placeholders(value, style);
      if (want !== got) {
        errors.push(
          `${label}/${locale}: placeholder mismatch ${key} (zh: ${want || "-"}, ${locale}: ${got || "-"})`,
        );
      }
      if (locale === "en" || locale === "es") {
        if (/[㐀-鿿豈-﫿぀-ヿ가-힯]/.test(value)) {
          errors.push(`${label}/${locale}: contains CJK characters: ${key}`);
        }
      }
      if (extraValues) extraValues({ label, locale, key, value });
    }
  }

  // 前端 vue-i18n：@ 是 linked message、| 是复数分隔符；本轮不用复数，直接拦。
  if (style === "vue") {
    for (const [locale, flat] of catalogs) {
      for (const [key, value] of flat) {
        if (/[@|]/.test(value)) {
          errors.push(`${label}/${locale}: unescaped @ or | in ${key}`);
        }
        if (/%\{\w+\}/.test(value)) {
          errors.push(`${label}/${locale}: rust placeholder %{...} in ${key}`);
        }
      }
    }
  }
  // 后端 rust-i18n：插值必须是 %{name}，裸 { 会被当成普通字符。
  if (style === "rust") {
    for (const [locale, flat] of catalogs) {
      for (const [key, value] of flat) {
        if (/(?<!%)\{\w+\}/.test(value)) {
          errors.push(`${label}/${locale}: bare {name} placeholder in ${key}`);
        }
      }
    }
  }

  const used = new Set();
  if (scanDir && fs.existsSync(scanDir)) {
    for (const file of walk(scanDir)) {
      const text = fs.readFileSync(file, "utf8");
      for (const match of text.matchAll(scanRe)) {
        const key = match[1] || match[2];
        if (!key || !key.includes(".")) continue;
        used.add(key);
        if (!base.has(key)) {
          errors.push(
            `${label}: referenced but missing in zh-CN: ${path.relative(root, file)}: ${key}`,
          );
        }
      }
    }
  }
  const unused = new Set([...base.keys()].filter((key) => !used.has(key)));
  return { catalogs, unused };
}

const frontend = checkCatalogSet({
  label: "frontend",
  dir: path.join(frontendRoot, "i18n", "locales"),
  style: "vue",
  scanDir: frontendRoot,
  scanRe: /(?:\$t|\bt)\(\s*["'`]([^"'`]+)["'`]|titleKey:\s*["'`]([^"'`]+)["'`]/g,
});

const backend = checkCatalogSet({
  label: "backend",
  dir: path.join(root, "src-tauri", "locales"),
  style: "rust",
  scanDir: rustRoot,
  scanRe: /\bt!\(\s*"([^"]+)"/g,
});

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}

console.log(
  `i18n ok: frontend ${frontend.catalogs.get("zh-CN").size} keys x ${locales.length}, ` +
    `backend ${backend.catalogs.get("zh-CN").size} keys x ${locales.length}`,
);

for (const [label, unused] of [
  ["frontend", frontend.unused],
  ["backend", backend.unused],
]) {
  if (!unused.size) continue;
  warnings.push(`unreferenced ${label} keys (may be dynamic or reserved):`);
  for (const key of [...unused].sort()) warnings.push("  " + key);
}
if (warnings.length) console.log(warnings.join("\n"));
