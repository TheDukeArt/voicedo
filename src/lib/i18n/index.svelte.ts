// Локаль интерфейса: общий каталог с Rust (`en.json`/`ru.json`, одно на всех).
// Источник истины разрешённой локали — Rust (`get_locale`/`locale-changed`);
// здесь только хранение, подстановка `{name}` и плюрализация через Intl.

import en from './en.json';
import ru from './ru.json';

export type ResolvedLocale = 'en' | 'ru' | 'zh';

type Catalog = Record<string, unknown>;

const catalogs: Record<ResolvedLocale, Catalog> = {
  en: en as Catalog,
  ru: ru as Catalog,
  // zh-каталога пока нет — t() даёт EN-фолбэк (ожидание этапа 11)
  zh: en as Catalog,
};

export const i18n = $state({ locale: 'en' as ResolvedLocale });

const active = $derived(catalogs[i18n.locale] ?? catalogs.en);

function lookup(catalog: Catalog, key: string): string | undefined {
  let cur: unknown = catalog;
  for (const part of key.split('.')) {
    if (typeof cur !== 'object' || cur === null) return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return typeof cur === 'string' ? cur : undefined;
}

function format(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) =>
    name in params ? String(params[name]) : m,
  );
}

export function t(key: string, params?: Record<string, string | number>): string {
  const raw = lookup(active, key) ?? lookup(catalogs.en, key) ?? key;
  return format(raw, params);
}

const pluralRules = $derived(new Intl.PluralRules(i18n.locale));

// Формы из объекта `{key}.one/.few/.many/.other` (RU 1/2-4/5+, EN one/other, ZH other).
export function tPlural(key: string, n: number): string {
  const cat = pluralRules.select(n);
  return lookup(active, `${key}.${cat}`) ?? lookup(active, `${key}.other`) ?? key;
}

const numberFormat = $derived(new Intl.NumberFormat(i18n.locale));

export function formatInt(n: number): string {
  return numberFormat.format(n);
}

export function formatDecimal(n: number, digits = 1): string {
  return new Intl.NumberFormat(i18n.locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(n);
}
