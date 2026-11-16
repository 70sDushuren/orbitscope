// OrbitScope 前端入口。

import { api } from './api.js';
import { clearSaved, fillForm, loadSaved, readForm, save, toQuery } from './filter.js';
import { Globe, loadCesium } from './globe.js';
import {
  n, renderDetails, renderHistogram, renderLive, renderSources, renderTypeBars, renderTypeChecks,
} from './panels.js';
import { readSettings, renderFetches, renderProvenance, renderSettings, updateCollectHint } from './settings.js';
import { Stream } from './stream.js';

const $ = (id) => document.getElementById(id);
const BIN_KM = 50;
const isNarrow = () => window.matchMedia('(max-width: 720px)').matches;

/** 窄屏下左右面板共用底部区域,同一时间只显示一个。 */
function setLeftVisible(visible) {
  $('left').hidden = !visible;
  $('btn-toggle-left').setAttribute('aria-expanded', String(visible));
}

const state = {
  filter: null,
  defaultFilter: null,
  generation: 0,
  catalogTotal: 0,
  selected: null, // { i, id, data, inSubscription }
  showTrack: true,
};

const detailDom = {
  panel: $('right'), name: $('d-name'), type: $('d-type'), static: $('d-static'),
  latlon: $('d-latlon'), alt: $('d-alt'), speed: $('d-speed'),
};

function setConn(connected) {
  $('st-conn-dot').className = `dot ${connected ? 'ok' : 'bad'}`;
  $('st-conn').textContent = connected ? '已连接' : '重连中';
}

function fail(msg) {
  const l = $('loading');
  l.hidden = false;
  l.classList.add('error');
  l.textContent = msg;
}

async function refreshStats() {
  try {
    const s = await api.stats(toQuery(state.filter), BIN_KM);
    $('stats-total').textContent = `共 ${n(s.total)} 个`;
    renderTypeBars($('type-bars'), s.by_type);
    $('hist-bin').textContent = BIN_KM;
    renderHistogram($('histogram'), $('hist-table'), s.altitude_histogram, BIN_KM, $('tooltip'));
  } catch (e) {
    $('stats-total').textContent = `统计失败:${e.message}`;
  }
}

async function refreshStatus() {
  try {
    const s = await api.status();
    state.catalogTotal = s.objects;
    state.settings = s.settings;
    $('st-catalog').textContent = n(s.objects);
    renderSources($('sources'), $('refresh-info'), s);
    renderProvenance($('provenance'), $('settings-persist'), s);
    renderFetches($('fetches-wrap'), $('fetches'), s.recent_fetches || []);
  } catch {
    /* 下次再试 */
  }
}

function applyFilter(f, { persist = true } = {}) {
  state.filter = f;
  if (persist) save(f);
  stream.subscribe(f, 1);
  refreshStats();
}

// ---------------------------------------------------------------- 选中物体

async function selectObject(i, norad) {
  globe.select(i ?? null);
  try {
    const data = await api.object(norad);
    state.selected = { i, id: norad, data };
    renderDetails(detailDom, data);
    detailDom.panel.hidden = false;
    if (isNarrow()) setLeftVisible(false);
    updateLive(true);
    if (state.showTrack) {
      const period = data.summary.period_min;
      const tr = await api.track(norad, Number.isFinite(period) ? Math.min(period, 1440).toFixed(1) : 90, 30);
      if (state.selected && state.selected.id === norad) globe.showTrack(tr.points, data.summary.object_type);
    }
  } catch (e) {
    console.warn(e);
  }
}

function closeDetails() {
  state.selected = null;
  globe.select(null);
  detailDom.panel.hidden = true;
}

let lastLive = 0;
let liveTimer = null;
function updateLive(force = false) {
  const sel = state.selected;
  if (!sel) return;
  const now = performance.now();
  if (!force && now - lastLive < 250) return;
  lastLive = now;
  const C = globe.C;
  const pos = sel.i !== null && sel.i !== undefined ? globe.positionOf(sel.i) : null;
  if (pos) {
    const c = C.Cartographic.fromCartesian(pos);
    const st = sel.data.state;
    renderLive(detailDom, {
      lat: C.Math.toDegrees(c.latitude), lon: C.Math.toDegrees(c.longitude), alt: c.height / 1000,
      speed: st ? st.speed_kms : null,
    });
  } else if (sel.data.state) {
    // 不在当前订阅中:使用 REST 结果,并定时刷新
    const g = sel.data.state.geodetic;
    renderLive(detailDom, { lat: g.lat_deg, lon: g.lon_deg, alt: g.alt_km, speed: sel.data.state.speed_kms });
    clearTimeout(liveTimer);
    liveTimer = setTimeout(async () => {
      if (state.selected !== sel) return;
      try {
        sel.data = await api.object(sel.id);
        updateLive(true);
      } catch { /* 忽略 */ }
    }, 2000);
  } else {
    renderLive(detailDom, null);
  }
}

// ---------------------------------------------------------------- 启动

let globe;
let stream;

async function main() {
  renderTypeChecks($('type-checks'));
  const form = $('filter-form');

  let cfg;
  try {
    cfg = await api.clientConfig();
    state.defaultFilter = await api.defaultFilter();
  } catch (e) {
    fail(`无法连接服务:${e.message}`);
    return;
  }

  try {
    const Cesium = await loadCesium(cfg.cesium_base_url);
    globe = new Globe(Cesium, $('globe'));
  } catch (e) {
    fail(`${e.message}。如需离线使用,请在配置中设置 server.cesium_dir 指向本地 Cesium 目录。`);
    return;
  }
  $('loading').hidden = true;

  stream = new Stream({
    state: setConn,
    hello: () => {},
    catalog: (msg) => {
      state.generation = msg.generation;
      const t0 = performance.now();
      globe.setCatalog(msg.objects);
      state.lastCatalogMs = performance.now() - t0;
      $('st-shown').textContent = `${n(msg.total)} 个`;
      if (state.selected) {
        const o = msg.objects.find((x) => x.id === state.selected.id);
        state.selected.i = o ? o.i : null;
        globe.select(state.selected.i);
      }
      refreshStats();
    },
    frame: (f) => {
      if (f.generation !== (state.generation >>> 0)) return;
      const t0 = performance.now();
      const shown = globe.applyFrame(f);
      state.lastFrameMs = performance.now() - t0;
      if (state.filter && state.filter.altitude_mode === 'instantaneous') $('st-shown').textContent = `${n(shown)} 个`;
    },
    error: (msg) => {
      $('filter-error').textContent = msg.message;
    },
  });
  stream.connect();
  // 调试入口(浏览器控制台中可用)
  window.orbitscope = { globe, stream, state };

  const saved = loadSaved();
  const initial = saved ? { ...state.defaultFilter, ...saved } : state.defaultFilter;
  fillForm(form, initial);
  try {
    applyFilter(readForm(form), { persist: false });
  } catch {
    fillForm(form, state.defaultFilter);
    applyFilter(readForm(form), { persist: false });
  }

  // 时间与数据设置
  const sform = $('settings-form');
  try {
    state.settings = await api.settings();
    renderSettings(sform, state.settings);
  } catch (e) {
    $('settings-error').textContent = `无法读取设置:${e.message}`;
  }
  sform.collect_mode.addEventListener('change', () => updateCollectHint(sform));
  for (const r of sform.querySelectorAll('input[name=time_mode]')) {
    r.addEventListener('change', () => (sform.fixed_time.disabled = r.value !== 'fixed' || !r.checked));
  }
  sform.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    $('settings-error').textContent = '';
    try {
      const patch = readSettings(sform, state.settings);
      if (Object.keys(patch).length === 0) return;
      state.settings = await api.putSettings(patch);
      renderSettings(sform, state.settings);
      // 服务端在后台按新设置重建编目;稍后刷新状态与统计
      setTimeout(refreshStatus, 800);
      setTimeout(() => {
        refreshStatus();
        refreshStats();
      }, 2500);
    } catch (e) {
      $('settings-error').textContent = e.message;
    }
  });

  form.addEventListener('submit', (ev) => {
    ev.preventDefault();
    $('filter-error').textContent = '';
    try {
      applyFilter(readForm(form));
    } catch (e) {
      $('filter-error').textContent = e.message;
    }
  });
  $('btn-reset').addEventListener('click', () => {
    clearSaved();
    fillForm(form, state.defaultFilter);
    $('filter-error').textContent = '';
    applyFilter(readForm(form), { persist: false });
  });

  // 时间显示
  setInterval(() => {
    const t = globe.displayTimeMs();
    $('st-time').textContent = t ? new Date(t).toISOString().slice(0, 19).replace('T', ' ') : '--';
    updateLive();
  }, 250);

  refreshStatus();
  setInterval(refreshStatus, 30000);

  // 拾取与悬停提示
  const C = globe.C;
  const handler = new C.ScreenSpaceEventHandler(globe.viewer.scene.canvas);
  handler.setInputAction((click) => {
    const i = globe.pick(click.position);
    if (i === null || i === undefined) return;
    const o = globe.objects.get(i);
    if (o) selectObject(i, o.id);
  }, C.ScreenSpaceEventType.LEFT_CLICK);

  const tip = $('tooltip');
  let pending = null;
  handler.setInputAction((move) => {
    pending = move.endPosition;
  }, C.ScreenSpaceEventType.MOUSE_MOVE);
  globe.viewer.scene.postRender.addEventListener(() => {
    if (!pending) return;
    const p = pending;
    pending = null;
    const i = globe.pick(p);
    const o = i !== null && i !== undefined ? globe.objects.get(i) : null;
    if (o) {
      tip.textContent = `${o.name}(${o.id})`;
      const rect = globe.viewer.scene.canvas.getBoundingClientRect();
      tip.style.left = `${rect.left + p.x + 12}px`;
      tip.style.top = `${rect.top + p.y + 12}px`;
      tip.hidden = false;
      globe.viewer.scene.canvas.style.cursor = 'pointer';
    } else {
      tip.hidden = true;
      globe.viewer.scene.canvas.style.cursor = '';
    }
  });

  // 详情面板按钮
  $('btn-close').addEventListener('click', closeDetails);
  $('btn-fly').addEventListener('click', () => state.selected && state.selected.i !== null && globe.flyTo(state.selected.i));
  $('btn-track').addEventListener('click', (ev) => {
    state.showTrack = !state.showTrack;
    ev.currentTarget.setAttribute('aria-pressed', String(state.showTrack));
    if (!state.showTrack) globe.clearTrack();
    else if (state.selected) selectObject(state.selected.i, state.selected.id);
  });

  // 面板开关
  $('btn-toggle-left').addEventListener('click', () => {
    const show = $('left').hidden;
    setLeftVisible(show);
    if (show && isNarrow()) detailDom.panel.hidden = true;
  });
  if (isNarrow()) setLeftVisible(false);

  // 搜索
  const input = $('search');
  const results = $('search-results');
  let timer = null;
  input.addEventListener('input', () => {
    clearTimeout(timer);
    const q = input.value.trim();
    if (!q) {
      results.replaceChildren();
      return;
    }
    timer = setTimeout(async () => {
      try {
        const r = await api.search(q);
        if (input.value.trim() !== q) return;
        results.replaceChildren(
          ...r.items.map((it) => {
            const li = document.createElement('li');
            const b = document.createElement('button');
            b.type = 'button';
            const name = document.createElement('span');
            name.textContent = it.name;
            const id = document.createElement('span');
            id.className = 'id';
            id.textContent = it.norad_id;
            b.append(name, id);
            b.addEventListener('click', () => {
              let idx = null;
              for (const [i, o] of globe.objects) if (o.id === it.norad_id) idx = i;
              selectObject(idx, it.norad_id).then(() => idx !== null && globe.flyTo(idx));
            });
            li.append(b);
            return li;
          }),
        );
        if (!r.items.length) results.innerHTML = '<li class="muted small">没有匹配的物体</li>';
      } catch (e) {
        results.innerHTML = '';
        const li = document.createElement('li');
        li.className = 'muted small';
        li.textContent = `搜索失败:${e.message}`;
        results.append(li);
      }
    }, 300);
  });
}

main();
