// 过滤条件:表单 ⇄ Filter JSON(WS 订阅用)⇄ REST 查询串。
import { t } from './i18n.js';


export const TYPES = [
  { key: 'PAYLOAD', param: 'payload', color: 'var(--type-payload)', get label() { return t('type.PAYLOAD'); } },
  { key: 'ROCKET BODY', param: 'rocket-body', color: 'var(--type-rocket)', get label() { return t('type.ROCKET BODY'); } },
  { key: 'DEBRIS', param: 'debris', color: 'var(--type-debris)', get label() { return t('type.DEBRIS'); } },
  { key: 'UNKNOWN', param: 'unknown', color: 'var(--type-unknown)', get label() { return t('type.UNKNOWN'); } },
];

export function typeInfo(key) {
  return TYPES.find((t) => t.key === key) || TYPES[3];
}

/** 用 Filter JSON 填充表单。 */
export function fillForm(form, f) {
  form.alt_min_km.value = f.alt_min_km;
  form.alt_max_km.value = f.alt_max_km;
  form.altitude_mode.value = f.altitude_mode;
  form.min_size_cm.value = f.min_size_cm;
  form.max_epoch_age_days.value = f.max_epoch_age_days ?? '';
  form.unknown_include.checked = f.unknown_size !== 'exclude';
  form.countries.value = (f.countries || []).join(', ');
  const selected = new Set(f.object_types && f.object_types.length ? f.object_types : TYPES.map((t) => t.key));
  for (const t of TYPES) {
    const box = form.querySelector(`input[name="type"][value="${t.key}"]`);
    if (box) box.checked = selected.has(t.key);
  }
}

/** 从表单读出 Filter JSON;非法时抛出带中文说明的错误。 */
export function readForm(form) {
  const num = (name, label) => {
    const v = Number(form[name].value);
    if (form[name].value === '' || !Number.isFinite(v)) throw new Error(t('filter.err.number', { field: label }));
    return v;
  };
  const alt_min_km = num('alt_min_km', t('filter.altMin'));
  const alt_max_km = num('alt_max_km', t('filter.altMax'));
  if (alt_min_km > alt_max_km) throw new Error(t('filter.err.altOrder'));
  const min_size_cm = num('min_size_cm', t('filter.minSize'));
  if (min_size_cm < 0) throw new Error(t('filter.err.sizeNegative'));
  const ageRaw = form.max_epoch_age_days.value.trim();
  const max_epoch_age_days = ageRaw === '' ? null : Number(ageRaw);
  if (max_epoch_age_days !== null && !(max_epoch_age_days >= 0)) throw new Error(t('filter.err.age'));
  const types = [...form.querySelectorAll('input[name="type"]:checked')].map((b) => b.value);
  if (types.length === 0) throw new Error(t('filter.err.noType'));
  return {
    alt_min_km,
    alt_max_km,
    altitude_mode: form.altitude_mode.value,
    min_size_cm,
    unknown_size: form.unknown_include.checked ? 'include' : 'exclude',
    // 全选时传空数组(= 全部)
    object_types: types.length === TYPES.length ? [] : types,
    countries: form.countries.value.split(/[,,\s]+/).map((s) => s.trim()).filter(Boolean),
    max_epoch_age_days,
  };
}

/** Filter JSON → REST 查询串。 */
export function toQuery(f) {
  const p = new URLSearchParams({
    alt_min: f.alt_min_km,
    alt_max: f.alt_max_km,
    alt_mode: f.altitude_mode,
    min_size_cm: f.min_size_cm,
    unknown_size: f.unknown_size,
  });
  if (f.object_types.length) p.set('types', f.object_types.map((k) => typeInfo(k).param).join(','));
  if (f.countries.length) p.set('countries', f.countries.join(','));
  if (f.max_epoch_age_days === null) p.set('any_age', 'true');
  else p.set('max_age_days', f.max_epoch_age_days);
  return p.toString();
}

const STORAGE_KEY = 'orbitscope.filter.v1';

export function loadSaved() {
  try {
    const s = localStorage.getItem(STORAGE_KEY);
    return s ? JSON.parse(s) : null;
  } catch {
    return null;
  }
}

export function save(f) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(f));
  } catch {
    /* 隐私模式等情况下忽略 */
  }
}

export function clearSaved() {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* 忽略 */
  }
}
