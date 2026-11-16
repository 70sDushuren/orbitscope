// "时间与数据"面板:实时/指定时刻、时刻策略、采集策略、数据依据、采集记录。

const COLLECT_HINT = {
  serve: '本 Web 服务每小时检查一次数据源并采集(仍受各数据源最短间隔限制)。',
  daemon: '本 Web 服务不联网,只读取本地历史;需另外常驻运行 `orbitscope collect`(见 deploy/ 中的自启配置)。',
  external: '本 Web 服务不联网,只读取本地历史;需由系统定时任务运行 `orbitscope fetch`(见 deploy/ 中的示例)。',
};

const POLICY_TEXT = {
  'known-at': (t) => `截至 ${t} 已获取的数据`,
  'nearest-epoch': (t) => `历元最接近 ${t} 的数据`,
  latest: () => '历史库中最新的数据',
};

const fmt = (iso) => (iso ? iso.replace('T', ' ').replace(/\.\d+/, '').replace('Z', '') : '-');

function fillSelect(sel, choices, value) {
  sel.replaceChildren(
    ...choices.map((c) => {
      const o = document.createElement('option');
      o.value = c.id;
      o.textContent = c.label;
      return o;
    }),
  );
  sel.value = value;
}

/** 用服务端设置填充表单。 */
export function renderSettings(form, s) {
  fillSelect(form.time_policy, s.policies, s.time_policy);
  fillSelect(form.collect_mode, s.collect_modes, s.collect_mode);
  form.time_policy.disabled = !s.history_enabled;
  for (const r of form.querySelectorAll('input[name=time_mode]')) r.checked = r.value === s.time_mode;
  // datetime-local 不带时区:按 UTC 显示与解释
  form.fixed_time.value = s.server_time.slice(0, 19);
  form.fixed_time.disabled = s.time_mode !== 'fixed';
  updateCollectHint(form);
}

export function updateCollectHint(form) {
  document.getElementById('collect-hint').textContent = COLLECT_HINT[form.collect_mode.value] || '';
}

/** 读出要提交的设置(只包含变化的字段)。 */
export function readSettings(form, current) {
  const patch = {};
  if (form.time_policy.value !== current.time_policy) patch.time_policy = form.time_policy.value;
  if (form.collect_mode.value !== current.collect_mode) patch.collect_mode = form.collect_mode.value;
  const mode = form.querySelector('input[name=time_mode]:checked')?.value || 'live';
  if (mode === 'live') {
    if (current.time_mode !== 'live') patch.time = 'live';
  } else {
    const v = form.fixed_time.value;
    if (!v) throw new Error('请填写指定时刻');
    const iso = `${v.length === 16 ? `${v}:00` : v}Z`;
    if (Number.isNaN(Date.parse(iso))) throw new Error('指定时刻格式不正确');
    patch.time = iso;
  }
  return patch;
}

/** 数据依据说明。 */
export function renderProvenance(el, persistEl, status) {
  const p = status.provenance;
  const s = status.settings;
  if (!s.history_enabled) {
    el.textContent = '历史归档未启用:显示数据源的最新数据。';
  } else if (!p) {
    el.textContent = '尚无数据依据(等待首次加载)。';
  } else if (p.records === 0) {
    el.textContent = `数据依据:${POLICY_TEXT[p.policy](fmt(p.data_time))}——没有数据(该时刻可能早于第一次采集)。`;
  } else {
    el.textContent =
      `数据依据:${POLICY_TEXT[p.policy](fmt(p.data_time))},选取 ${p.records.toLocaleString("zh-CN")} 组根数;` +
      `根数获取于 ${fmt(p.first_seen_min)} 至 ${fmt(p.first_seen_max)}(UTC)。`;
  }
  persistEl.textContent = s.persisted ? '设置会保存,重启后仍生效(时间除外)。' : '设置只在本次运行有效。';
  document.getElementById('st-replay').hidden = s.time_mode !== 'fixed';
}

const HOW = { network: '下载', not_modified: '确认未更新' };

/** 历史库中的采集记录。 */
export function renderFetches(wrap, table, rows) {
  wrap.hidden = rows.length === 0;
  table.querySelector('tbody').replaceChildren(
    ...rows.map((r) => {
      const tr = document.createElement('tr');
      for (const text of [fmt(r.fetched_at), r.resource, HOW[r.origin] || r.origin]) {
        const td = document.createElement('td');
        td.textContent = text;
        tr.append(td);
      }
      return tr;
    }),
  );
}
