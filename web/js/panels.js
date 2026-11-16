// 面板渲染:统计(类型条形 + 高度直方图)、数据源、物体详情。

import { TYPES, typeInfo } from './filter.js';

const fmt = new Intl.NumberFormat('zh-CN');
export const n = (v) => fmt.format(v);

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
    ...TYPES.map((t) =>
      el('label', { class: 'check' }, el('input', { type: 'checkbox', name: 'type', value: t.key, checked: '' }), swatch(t.color), t.label),
    ),
  );
}

/** 按类型的水平条形(颜色跟随类型,数值用文字色)。 */
export function renderTypeBars(container, byType) {
  const max = Math.max(1, ...TYPES.map((t) => byType[t.key] || 0));
  container.replaceChildren(
    ...TYPES.map((t) => {
      const v = byType[t.key] || 0;
      return el(
        'div',
        { class: 'tb-row', title: `${t.label}:${n(v)}` },
        el('span', { class: 'tb-name' }, swatch(t.color), t.label),
        el('div', { class: 'tb-track' }, el('div', { class: 'tb-fill', style: `width:${(v / max) * 100}%;background:${t.color}` })),
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
    container.append(el('p', { class: 'muted small' }, '没有数据'));
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
  const root = svg('svg', { viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': '平均高度分布直方图' });
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
      tooltip.textContent = `${lo}–${lo + binKm} km:${n(c)}`;
      tooltip.hidden = false;
      tooltip.style.left = `${ev.clientX + 12}px`;
      tooltip.style.top = `${ev.clientY + 12}px`;
    });
    hit.addEventListener('mouseleave', () => (tooltip.hidden = true));
    root.append(hit);
  });
  root.append(svg('line', { x1: 0, x2: W, y1: H - padB + 0.5, y2: H - padB + 0.5, class: 'baseline' }));
  const label = (x, text, anchor) => {
    const t = svg('text', { x, y: H - 3, class: 'axis', 'text-anchor': anchor });
    t.textContent = text;
    root.append(t);
  };
  label(0, `${lo0} km`, 'start');
  label(W, `${hiN + binKm} km`, 'end');
  container.append(root);
}

function ago(iso) {
  const s = (Date.now() - Date.parse(iso)) / 1000;
  if (!Number.isFinite(s)) return '';
  if (s < 120) return `${Math.round(s)} 秒前`;
  if (s < 7200) return `${Math.round(s / 60)} 分钟前`;
  if (s < 172800) return `${(s / 3600).toFixed(1)} 小时前`;
  return `${(s / 86400).toFixed(1)} 天前`;
}

const ORIGIN = { network: '已下载', not_modified: '服务器确认未更新', cache: '缓存', stale_cache: '旧缓存(联网失败)' };

/** 数据源状态列表(状态点 + 文字,不只依赖颜色)。 */
export function renderSources(list, info, status) {
  list.replaceChildren(
    ...status.sources.map((s) => {
      const stale = s.resources.some((r) => r.origin.kind === 'stale_cache');
      const cls = !s.ok ? 'bad' : stale ? 'warn' : 'ok';
      const res = s.resources[0];
      const detail = !s.ok
        ? `失败:${s.error}`
        : `${n(s.records)} 条${res ? `,${ORIGIN[res.origin.kind] || res.origin.kind},${ago(res.fetched_at)}` : ''}`;
      return el('li', {}, el('span', { class: `dot ${cls}`, 'aria-hidden': 'true' }), el('strong', {}, s.label), el('span', { class: 'muted' }, detail));
    }),
  );
  const sch = status.schedule;
  info.textContent = sch.next_refresh ? `下次检查更新:${new Date(sch.next_refresh).toLocaleString()}` : '';
}

function fmtNum(v, d) {
  return v === null || v === undefined || !Number.isFinite(v) ? '--' : v.toFixed(d);
}

function sizeText(s) {
  if (s.size_source === 'unknown') return '未知';
  if (s.size_source === 'numeric_rcs') return `≈${fmtNum(s.size_min_m, 2)} m(数值 RCS)`;
  if (s.size_min_m > 0 && s.size_max_m) return `${fmtNum(s.size_min_m, 2)}–${fmtNum(s.size_max_m, 2)} m(RCS 分档)`;
  if (s.size_max_m) return `< ${fmtNum(s.size_max_m, 2)} m(RCS 分档)`;
  return `> ${fmtNum(s.size_min_m, 2)} m(RCS 分档)`;
}

/** 详情面板静态部分。 */
export function renderDetails(dom, data) {
  const s = data.summary;
  const t = typeInfo(s.object_type);
  dom.name.textContent = s.name;
  dom.type.replaceChildren(swatch(t.color), `${t.label} · NORAD ${s.norad_id}`);
  const rows = [
    ['国际编号', s.intl_designator || '--'],
    ['国家/所有者', s.country || '--'],
    ['近地点 / 远地点', `${fmtNum(s.perigee_km, 0)} / ${fmtNum(s.apogee_km, 0)} km`],
    ['倾角', `${fmtNum(s.inclination_deg, 2)}°`],
    ['周期', `${fmtNum(s.period_min, 1)} 分钟`],
    ['估算尺寸', sizeText(s)],
    ['根数历元', s.epoch.replace('T', ' ').replace('Z', '')],
    ['根数龄期', `${fmtNum(s.epoch_age_days, 1)} 天`],
    ['数据来源', s.source],
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
