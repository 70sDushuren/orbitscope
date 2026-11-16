// "时间与数据"面板:实时/指定时刻、时刻策略、采集策略、数据依据、采集记录。

import { fmtNumber, t } from './i18n.js';

const fmt = (iso) => (iso ? iso.replace('T', ' ').replace(/\.\d+/, '').replace('Z', '') : '-');

function fillSelect(sel, choices, value, prefix) {
  sel.replaceChildren(
    ...choices.map((c) => {
      const o = document.createElement('option');
      o.value = c.id;
      o.textContent = t(`${prefix}.${c.id}`);
      return o;
    }),
  );
  sel.value = value;
}

/**
 * 用服务端设置填充表单。时间不是服务端设置:`time` = { mode: 'live' | 'fixed', ms } 是本页面自己的状态。
 */
export function renderSettings(form, s, time) {
  fillSelect(form.time_policy, s.policies, s.time_policy, 'policy');
  fillSelect(form.collect_mode, s.collect_modes, s.collect_mode, 'collect');
  form.time_policy.disabled = !s.history_enabled;
  for (const r of form.querySelectorAll('input[name=time_mode]')) r.checked = r.value === time.mode;
  // datetime-local 不带时区:按 UTC 显示与解释
  form.fixed_time.value = new Date(time.ms).toISOString().slice(0, 19);
  form.fixed_time.disabled = time.mode !== 'fixed';
  updateCollectHint(form);
}

export function updateCollectHint(form) {
  document.getElementById('collect-hint').textContent = form.collect_mode.value ? t(`collectHint.${form.collect_mode.value}`) : '';
}

/** 读出要提交给服务端的设置(只包含变化的字段)。 */
export function readSettings(form, current) {
  const patch = {};
  if (form.time_policy.value !== current.time_policy) patch.time_policy = form.time_policy.value;
  if (form.collect_mode.value !== current.collect_mode) patch.collect_mode = form.collect_mode.value;
  return patch;
}

/** 读出表单中的时间:`null` = 实时;否则为指定时刻(UTC 毫秒)。 */
export function readTime(form) {
  const mode = form.querySelector('input[name=time_mode]:checked')?.value || 'live';
  if (mode === 'live') return null;
  const v = form.fixed_time.value;
  if (!v) throw new Error(t('settings.needTime'));
  const ms = Date.parse(`${v.length === 16 ? `${v}:00` : v}Z`);
  if (Number.isNaN(ms)) throw new Error(t('settings.badTime'));
  return ms;
}

/** 数据依据说明。 */
export function renderProvenance(el, persistEl, status) {
  const p = status.provenance;
  const s = status.settings;
  if (!s.history_enabled) {
    el.textContent = t('prov.disabled');
  } else if (!p) {
    el.textContent = t('prov.waiting');
  } else {
    const what = t(`prov.${p.policy}`, { time: fmt(p.data_time) });
    el.textContent =
      p.records === 0
        ? t('prov.empty', { what })
        : t('prov.full', {
            what,
            count: fmtNumber(p.records),
            from: fmt(p.first_seen_min),
            to: fmt(p.first_seen_max),
          });
  }
  persistEl.textContent = s.persisted ? t('settings.persisted') : t('settings.notPersisted');
}


/** 历史库中的采集记录。 */
export function renderFetches(wrap, table, rows) {
  wrap.hidden = rows.length === 0;
  table.querySelector('tbody').replaceChildren(
    ...rows.map((r) => {
      const tr = document.createElement('tr');
      for (const text of [fmt(r.fetched_at), r.resource, t(`how.${r.origin}`)]) {
        const td = document.createElement('td');
        td.textContent = text;
        tr.append(td);
      }
      return tr;
    }),
  );
}
