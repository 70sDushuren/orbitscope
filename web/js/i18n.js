// 界面多语言。词典在 web/i18n/<语言>.json;简体中文(zh-CN)为源语言,其余语言由
// scripts/i18n/i18n.mjs 在发布时检查并自动补译。
//
// HTML 中用属性标记需要翻译的位置:
//   data-i18n="key"              → textContent
//   data-i18n-placeholder="key"  → placeholder
//   data-i18n-aria-label="key"   → aria-label
//   data-i18n-title="key"        → title

export const LOCALES = [
  { id: 'zh-CN', name: '简体中文' },
  { id: 'zh-HK', name: '繁體中文(香港)' },
  { id: 'en', name: 'English' },
];
const SOURCE = 'zh-CN';
const STORAGE_KEY = 'orbitscope.locale';

let current = SOURCE;
let dict = {};
let fallback = {};

/** 按浏览器语言选择:zh-HK / zh-TW / zh-Hant → 繁體;其他 zh → 简体;其余 → English。 */
export function detectLocale(langs = navigator.languages || [navigator.language]) {
  for (const raw of langs) {
    const l = String(raw || '').toLowerCase();
    if (/^zh-(hk|mo|tw)|hant/.test(l)) return 'zh-HK';
    if (l.startsWith('zh')) return 'zh-CN';
    if (l.startsWith('en')) return 'en';
  }
  return 'en';
}

function saved() {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return LOCALES.some((l) => l.id === v) ? v : null;
  } catch {
    return null;
  }
}

async function load(id) {
  const resp = await fetch(`i18n/${id}.json`, { cache: 'no-cache' });
  if (!resp.ok) throw new Error(`无法加载语言包 ${id}: HTTP ${resp.status}`);
  return resp.json();
}

export function locale() {
  return current;
}

/** 初始化:读取保存的选择或浏览器语言。语言包加载失败时回退到简体中文。 */
export async function initI18n() {
  fallback = await load(SOURCE);
  await setLocale(saved() || detectLocale(), { persist: false });
}

/** 切换语言并重新应用到页面;触发 `localechange` 事件供动态内容重新渲染。 */
export async function setLocale(id, { persist = true } = {}) {
  if (!LOCALES.some((l) => l.id === id)) id = SOURCE;
  try {
    dict = id === SOURCE ? fallback : await load(id);
    current = id;
  } catch (e) {
    console.warn(e);
    dict = fallback;
    current = SOURCE;
  }
  if (persist) {
    try {
      localStorage.setItem(STORAGE_KEY, current);
    } catch {
      /* 隐私模式下忽略 */
    }
  }
  applyI18n();
  window.dispatchEvent(new CustomEvent('localechange', { detail: current }));
}

/** 取翻译;缺失时依次回退到简体中文、键名。`{name}` 由 params 替换。 */
export function t(key, params) {
  let s = dict[key] ?? fallback[key] ?? key;
  if (params) s = s.replace(/\{(\w+)\}/g, (m, k) => (k in params ? String(params[k]) : m));
  return s;
}

/** 按当前语言格式化数字。 */
export function fmtNumber(v, opts) {
  return new Intl.NumberFormat(current === 'zh-HK' ? 'zh-HK' : current, opts).format(v);
}

/** 按当前语言格式化本地日期时间。 */
export function fmtDateTime(d) {
  return new Date(d).toLocaleString(current);
}

/** 把翻译应用到带 data-i18n* 属性的元素。 */
export function applyI18n(root = document) {
  document.documentElement.lang = current;
  for (const el of root.querySelectorAll('[data-i18n]')) el.textContent = t(el.dataset.i18n);
  for (const el of root.querySelectorAll('[data-i18n-placeholder]')) el.placeholder = t(el.dataset.i18nPlaceholder);
  for (const el of root.querySelectorAll('[data-i18n-aria-label]')) el.setAttribute('aria-label', t(el.dataset.i18nAriaLabel));
  for (const el of root.querySelectorAll('[data-i18n-title]')) el.title = t(el.dataset.i18nTitle);
}
