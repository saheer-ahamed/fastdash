// Frontend i18n. Strings live in the shared catalog under `locales/<lang>/*.json`
// (the same files the Rust backend embeds), so there is one source of truth per
// language. `t("section.key", { param })` resolves a dotted key and substitutes
// {param} placeholders. Add a language: create locales/<lang>/*.json and register
// it in `catalogs` below.

import enApp from "../locales/en/app.json";
import enClaude from "../locales/en/claude.json";
import enGithub from "../locales/en/github.json";
import enSlack from "../locales/en/slack.json";

type Catalog = Record<string, unknown>;

const catalogs: Record<string, Catalog> = {
  en: { ...enApp, ...enClaude, ...enGithub, ...enSlack },
};

let locale = "en";

// Selectable languages. Add a language: create locales/<id>/*.json, import and
// merge it into `catalogs` above, and add an entry here.
export const LOCALES: { id: string; label: string }[] = [{ id: "en", label: "English" }];

export function availableLocales(): string[] {
  return Object.keys(catalogs);
}

export function getLocale(): string {
  return locale;
}

export function setLocale(next: string): void {
  locale = catalogs[next] ? next : "en";
}

function lookup(cat: Catalog, key: string): unknown {
  return key.split(".").reduce<unknown>((node, seg) => {
    if (node && typeof node === "object") {
      return (node as Record<string, unknown>)[seg];
    }
    return undefined;
  }, cat);
}

export function t(key: string, params?: Record<string, string | number>): string {
  let value = lookup(catalogs[locale] ?? catalogs.en, key);
  if (typeof value !== "string") value = lookup(catalogs.en, key);
  if (typeof value !== "string") return key;

  let out = value;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      out = out.split(`{${k}}`).join(String(v));
    }
  }
  return out;
}
