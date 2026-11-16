// 面板渲染:统计(类型条形 + 高度直方图)、数据源、物体详情。

import { TYPES, typeInfo } from './filter.js';
import { errText, fmtDateTime, fmtNumber, t } from './i18n.js';

export const n = (v) => fmtNumber(v);

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'style') e.style.cssText = v;
    else if (k === 'class') e.className = v;
    else e.setAttribute(k, v);
  }
  for (const c of children) e.append(c);
  return e;
}

export function swatch(color) {
  return el('span', { class: 'swatch', style: `background:${color}`, 'aria-hidden': 'true' });
}

/** 类型复选框(带颜色标识)。 */
export function renderTypeChecks(container) {
  container.replaceChildren(
    ...TYPES.map((ty) =>
      el('label', { class: 'check' }, el('input', { type: 'checkbox', name: 'type', value: ty.key, checked: '' }), swatch(ty.color), ty.label),
    ),
  );
}

/** 按类型的水平条形(颜色跟随类型,数值用文字色)。 */
export function renderTypeBars(container, byType) {
  const max = Math.max(1, ...TYPES.map((ty) => byType[ty.key] || 0));
  container.replaceChildren(
    ...TYPES.map((ty) => {
      const v = byType[ty.key] || 0;
      return el(
        'div',
        { class: 'tb-row', title: `${ty.label}:${n(v)}` },
        el('span', { class: 'tb-name' }, swatch(ty.color), ty.label),
        el('div', { class: 'tb-track' }, el('div', { class: v ? 'tb-fill' : 'tb-fill zero', style: `width:${(v / max) * 100}%;background:${ty.color}` })),
        el('span', { class: 'tb-val' }, n(v)),
      );
    }),
  );
}

const SVG = 'http://www.w3.org/2000/svg';
function svg(tag, attrs) {
  const e = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

/** 高度直方图(SVG,单一色相),悬停显示数值;同时填充数据表。 */
export function renderHistogram(container, table, hist, binKm, tooltip) {
  container.replaceChildren();
  const tbody = table.querySelector('tbody');
  tbody.replaceChildren(
    ...hist.map(([lo, c]) => el('tr', {}, el('td', {}, `${lo}–${lo + binKm}`), el('td', {}, n(c)))),
  );
  if (!hist.length) {
    container.append(el('p', { class: 'muted small' }, t('stats.noData')));
    return;
  }
  // 补齐空分箱,使横轴连续
  const lo0 = hist[0][0];
  const hiN = hist[hist.length - 1][0];
  const counts = new Map(hist.map(([lo, c]) => [lo, c]));
  const bins = [];
  for (let lo = lo0; lo <= hiN + 1e-9; lo += binKm) bins.push([lo, counts.get(lo) || 0]);

  const W = 290, H = 110, padB = 16, gap = 2;
  const max = Math.max(...bins.map((b) => b[1]), 1);
  const bw = Math.max((W - gap * (bins.length - 1)) / bins.length, 1);
  const root = svg('svg', { viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': t('stats.histAria') });
  bins.forEach(([lo, c], k) => {
    const h = c ? Math.max(((H - padB) * c) / max, 2) : 0;
    const x = k * (bw + gap);
    const r = Math.min(4, bw / 2, h);
    // 顶部圆角、底部贴基线的柱形
    const y = H - padB - h;
    const d = h
      ? `M${x},${H - padB}V${y + r}Q${x},${y} ${x + r},${y}H${x + bw - r}Q${x + bw},${y} ${x + bw},${y + r}V${H - padB}Z`
      : '';
    if (d) {
      const bar = svg('path', { d, class: 'bar' });
      root.append(bar);
    }
    // 命中区域比柱形大:整列
    const hit = svg('rect', { x, y: 0, width: bw + gap, height: H - padB, fill: 'transparent' });
    hit.addEventListener('mousemove', (ev) => {
      tooltip.textContent = `${lo}–${lo + binKm} km: ${n(c)}`;
      tooltip.hidden = false;
      tooltip.style.left = `${ev.clientX + 12}px`;
      tooltip.style.top = `${ev.clientY + 12}px`;
    });
    hit.addEventListener('mouseleave', () => (tooltip.hidden = true));
    root.append(hit);
  });
  root.append(svg('line', { x1: 0, x2: W, y1: H - padB + 0.5, y2: H - padB + 0.5, class: 'baseline' }));
  const label = (x, text, anchor) => {
    const txt = svg('text', { x, y: H - 3, class: 'axis', 'text-anchor': anchor });
    txt.textContent = text;
    root.append(txt);
  };
  label(0, `${lo0} km`, 'start');
  label(W, `${hiN + binKm} km`, 'end');
  container.append(root);
}

function ago(iso) {
  const s = (Date.now() - Date.parse(iso)) / 1000;
  if (!Number.isFinite(s)) return '';
  if (s < 120) return t('ago.seconds', { n: Math.round(s) });
  if (s < 7200) return t('ago.minutes', { n: Math.round(s / 60) });
  if (s < 172800) return t('ago.hours', { n: (s / 3600).toFixed(1) });
  return t('ago.days', { n: (s / 86400).toFixed(1) });
}


/** 数据源状态列表(状态点 + 文字,不只依赖颜色)。 */
export function renderSources(list, info, status) {
  if (!status.sources.length && status.settings && status.settings.collect_mode !== 'serve') {
    list.replaceChildren(el('li', { class: 'muted' }, t('sources.passive')));
    info.textContent = '';
    return;
  }
  list.replaceChildren(
    ...status.sources.map((s) => {
      const stale = s.resources.some((r) => r.origin.kind === 'stale_cache');
      const cls = !s.ok ? 'bad' : stale ? 'warn' : 'ok';
      const res = s.resources[0];
      const detail = !s.ok
        ? t('sources.failed', { error: errText({ code: s.error_code ? `source_${s.error_code}` : null, params: { detail: s.error_detail }, message: s.error }) })
        : [t('sources.records', { n: n(s.records) }), ...(res ? [t(`origin.${res.origin.kind}`), ago(res.fetched_at)] : [])].join(' · ');
      return el('li', {}, el('span', { class: `dot ${cls}`, 'aria-hidden': 'true' }), el('strong', {}, s.label), el('span', { class: 'muted' }, detail));
    }),
  );
  const sch = status.schedule;
  info.textContent = sch.next_refresh
    ? t('sources.nextRefresh', { time: fmtDateTime(sch.next_refresh) })
    : sch.next_reload
      ? t('sources.nextReload', { time: fmtDateTime(sch.next_reload) })
      : '';
}

function fmtNum(v, d) {
  return v === null || v === undefined || !Number.isFinite(v) ? '--' : v.toFixed(d);
}

function sizeText(s) {
  if (s.size_source === 'unknown') return t('size.unknown');
  if (s.size_source === 'numeric_rcs') return t('size.numeric', { d: fmtNum(s.size_min_m, 2) });
  if (s.size_min_m > 0 && s.size_max_m) return t('size.range', { min: fmtNum(s.size_min_m, 2), max: fmtNum(s.size_max_m, 2) });
  if (s.size_max_m) return t('size.below', { max: fmtNum(s.size_max_m, 2) });
  return t('size.above', { min: fmtNum(s.size_min_m, 2) });
}

/** 详情面板静态部分。 */
export function renderDetails(dom, data) {
  const s = data.summary;
  const ti = typeInfo(s.object_type);
  dom.name.textContent = s.name;
  dom.type.replaceChildren(swatch(ti.color), `${ti.label} · NORAD ${s.norad_id}`);
  const rows = [
    [t('details.intl'), s.intl_designator || '--'],
    [t('details.country'), s.country || '--'],
    [t('details.perigeeApogee'), `${fmtNum(s.perigee_km, 0)} / ${fmtNum(s.apogee_km, 0)} km`],
    [t('details.inclination'), `${fmtNum(s.inclination_deg, 2)}°`],
    [t('details.period'), t('details.periodValue', { n: fmtNum(s.period_min, 1) })],
    [t('details.size'), sizeText(s)],
    [t('details.epoch'), s.epoch.replace('T', ' ').replace('Z', '')],
    [t('details.epochAge'), t('details.days', { n: fmtNum(s.epoch_age_days, 1) })],
    [t('details.source'), s.source],
  ];
  dom.static.replaceChildren(...rows.flatMap(([k, v]) => [el('dt', {}, k), el('dd', {}, v)]));
}

/** 详情面板实时部分。 */
export function renderLive(dom, live) {
  if (!live) {
    dom.latlon.textContent = '--';
    dom.alt.textContent = '--';
    dom.speed.textContent = '--';
    return;
  }
  dom.latlon.textContent = `${fmtNum(live.lat, 3)}° / ${fmtNum(live.lon, 3)}°`;
  dom.alt.textContent = `${fmtNum(live.alt, 1)} km`;
  dom.speed.textContent = live.speed === null ? '--' : `${fmtNum(live.speed, 3)} km/s`;
}
