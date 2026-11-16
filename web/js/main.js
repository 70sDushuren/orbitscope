// OrbitScope 前端入口。

import { api, getApiToken, setApiToken, setView } from './api.js';
import { LOCALES, errText, initI18n, locale, setLocale, t } from './i18n.js';
import { clearSaved, fillForm, loadSaved, readForm, save, toQuery } from './filter.js';
import { Globe, loadCesium } from './globe.js';
import {
  n, renderDetails, renderHistogram, renderLive, renderSources, renderTypeBars, renderTypeChecks,
} from './panels.js';
import { readSettings, readTime, renderFetches, renderProvenance, renderSettings, updateCollectHint } from './settings.js';
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
  // 本页面的时间模式('live' 实时 / 'fixed' 指定时刻)与对应视图编号,由服务端的 catalog 消息确认
  timeMode: 'live',
  view: null,
};

/** 设置表单用的时间状态:当前模式与正在显示的时刻。 */
function timeState() {
  return { mode: state.timeMode, ms: (globe && globe.displayTimeMs()) || Date.now() };
}

const detailDom = {
  panel: $('right'), name: $('d-name'), type: $('d-type'), static: $('d-static'),
  latlon: $('d-latlon'), alt: $('d-alt'), speed: $('d-speed'),
};

function setConn(connected) {
  $('st-conn-dot').className = `dot ${connected ? 'ok' : 'bad'}`;
  state.connected = connected;
  $('st-conn').textContent = connected ? t('top.connected') : t('top.reconnecting');
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
    state.lastStats = s;
    $('stats-total').textContent = t('stats.total', { n: n(s.total) });
    renderTypeBars($('type-bars'), s.by_type);
    $('hist-title').textContent = t('stats.histTitle', { bin: BIN_KM });
    renderHistogram($('histogram'), $('hist-table'), s.altitude_histogram, BIN_KM, $('tooltip'));
  } catch (e) {
    $('stats-total').textContent = t('stats.failed', { error: errText(e) });
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

/** 当前显示数量(用于语言切换后重绘)。 */
function setShown(count) {
  state.shownCount = count;
  $('st-shown').textContent = t('top.count', { n: n(count) });
}

/** 语言选择器。 */
function setupLanguage() {
  const sel = $('lang-select');
  sel.replaceChildren(
    ...LOCALES.map((l) => {
      const o = document.createElement('option');
      o.value = l.id;
      o.textContent = l.name;
      return o;
    }),
  );
  sel.value = locale();
  sel.addEventListener('change', () => setLocale(sel.value));
}

async function main() {
  try {
    await initI18n();
  } catch (e) {
    console.warn('语言包加载失败,使用页面内置文字', e);
  }
  setupLanguage();
  renderTypeChecks($('type-checks'));
  const form = $('filter-form');

  let cfg;
  try {
    cfg = await api.clientConfig();
    state.defaultFilter = await api.defaultFilter();
  } catch (e) {
    fail(t('app.cannotConnect', { error: errText(e) }));
    return;
  }

  try {
    const Cesium = await loadCesium(cfg.cesium_base_url, cfg.cesium_integrity);
    globe = new Globe(Cesium, $('globe'));
  } catch (e) {
    fail(t('app.cesiumFailed', { url: e.cesiumUrl || cfg.cesium_base_url }));
    return;
  }
  $('loading').hidden = true;

  stream = new Stream({
    state: setConn,
    hello: () => {},
    catalog: (msg) => {
      // 时间模式或视图变了(切换时间、断线重连):REST 请求改用新视图,并刷新依赖时刻的内容
      const viewChanged = (msg.view || null) !== state.view;
      state.view = msg.view || null;
      state.timeMode = msg.time_mode === 'fixed' ? 'fixed' : 'live';
      setView(state.view);
      $('st-replay').hidden = state.timeMode !== 'fixed';
      state.generation = msg.generation;
      const t0 = performance.now();
      globe.setCatalog(msg.objects);
      state.lastCatalogMs = performance.now() - t0;
      setShown(msg.total);
      if (state.selected) {
        const o = msg.objects.find((x) => x.id === state.selected.id);
        state.selected.i = o ? o.i : null;
        globe.select(state.selected.i);
      }
      refreshStats();
      if (viewChanged) {
        refreshStatus();
        if (state.selected) selectObject(state.selected.i, state.selected.id);
      }
    },
    frame: (f) => {
      if (f.generation !== (state.generation >>> 0)) return;
      const t0 = performance.now();
      const shown = globe.applyFrame(f);
      state.lastFrameMs = performance.now() - t0;
      if (state.filter && state.filter.altitude_mode === 'instantaneous') setShown(shown);
    },
    error: (msg) => {
      // 时间相关的错误显示在时间设置处,其余显示在过滤表单处
      $(msg.code === 'bad_time' ? 'settings-error' : 'filter-error').textContent = errText(msg);
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
  // 服务端要求 token(或本浏览器已保存过 token)时显示输入框
  const showTokenField = () => {
    $('token-field').hidden = false;
  };
  sform.api_token.value = getApiToken();
  if (sform.api_token.value) showTokenField();
  try {
    state.settings = await api.settings();
    renderSettings(sform, state.settings, timeState());
    if (state.settings.token_required) showTokenField();
  } catch (e) {
    $('settings-error').textContent = t('settings.loadFailed', { error: errText(e) });
  }
  // 时间只属于本页面:改动过才在"应用设置"时生效(否则会把正在走的时钟拨回去)
  let timeDirty = false;
  sform.fixed_time.addEventListener('input', () => (timeDirty = true));
  sform.collect_mode.addEventListener('change', () => updateCollectHint(sform));
  for (const r of sform.querySelectorAll('input[name=time_mode]')) {
    r.addEventListener('change', () => {
      timeDirty = true;
      sform.fixed_time.disabled = r.value !== 'fixed' || !r.checked;
    });
  }
  sform.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    $('settings-error').textContent = '';
    try {
      // 时间:只切换本页面的连接,不改服务端设置,也不需要 token
      if (timeDirty) {
        stream.setTime(readTime(sform));
        timeDirty = false;
      }
      if (!state.settings) return;
      setApiToken(sform.api_token.value.trim());
      const patch = readSettings(sform, state.settings);
      if (Object.keys(patch).length === 0) return;
      state.settings = await api.putSettings(patch);
      renderSettings(sform, state.settings, timeState());
      // 服务端在后台按新设置重建编目;稍后刷新状态与统计
      setTimeout(refreshStatus, 800);
      setTimeout(() => {
        refreshStatus();
        refreshStats();
      }, 2500);
    } catch (e) {
      if (e.code === 'token_required') {
        showTokenField();
        sform.api_token.focus();
      }
      $('settings-error').textContent = errText(e);
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
        if (!r.items.length) {
          const li = document.createElement('li');
          li.className = 'muted small';
          li.textContent = t('search.none');
          results.replaceChildren(li);
        }
      } catch (e) {
        results.innerHTML = '';
        const li = document.createElement('li');
        li.className = 'muted small';
        li.textContent = t('search.failed', { error: errText(e) });
        results.append(li);
      }
    }, 300);
  });

  // 切换语言:重绘所有动态生成的文字
  window.addEventListener('localechange', () => {
    $('lang-select').value = locale();
    renderTypeChecks($('type-checks'));
    if (state.filter) fillForm(form, state.filter);
    $('filter-error').textContent = '';
    $('settings-error').textContent = '';
    if (state.settings && !timeDirty) renderSettings(sform, state.settings, timeState());
    if (state.connected !== undefined) setConn(state.connected);
    if (state.shownCount !== undefined) setShown(state.shownCount);
    if (state.selected) renderDetails(detailDom, state.selected.data);
    results.replaceChildren();
    input.value = '';
    refreshStats();
    refreshStatus();
  });
}

main();
