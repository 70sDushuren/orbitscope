// REST API 客户端。

async function getJson(url, options) {
  const resp = await fetch(url, options);
  let body = null;
  try {
    body = await resp.json();
  } catch {
    /* 非 JSON 响应 */
  }
  if (!resp.ok) {
    const err = new Error((body && body.error) || `HTTP ${resp.status}`);
    err.status = resp.status;
    // 机器可读的错误代码(如 token_required、local_only),界面据此显示本地化说明
    err.code = (body && body.code) || null;
    err.params = (body && body.params) || {};
    throw err;
  }
  return body;
}

// 写接口用的 API token:只保存在本浏览器中(localStorage 不可用时仅本次页面有效)。
const TOKEN_KEY = 'orbitscope.apiToken';
let memoryToken = '';

export function getApiToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || memoryToken;
  } catch {
    return memoryToken;
  }
}

export function setApiToken(token) {
  memoryToken = token;
  try {
    if (token) localStorage.setItem(TOKEN_KEY, token);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* 隐私模式等情况下不可用 */
  }
}

function writeHeaders() {
  const headers = { 'Content-Type': 'application/json' };
  const token = getApiToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

// 本连接"指定时刻"视图的编号(由 WS 的 catalog 消息给出;实时为 null)。
// 带上它,REST 请求就使用与 3D 视图相同的时刻与数据。
let viewId = null;

export function setView(id) {
  viewId = id || null;
}

/** 给查询串加上 view 参数。 */
function withView(url) {
  if (!viewId) return url;
  return `${url}${url.includes('?') ? '&' : '?'}view=${encodeURIComponent(viewId)}`;
}

export const api = {
  clientConfig: () => getJson('/api/v1/client-config'),
  status: () => getJson(withView('/api/v1/status')),
  defaultFilter: () => getJson('/api/v1/filters/default'),
  object: (id) => getJson(withView(`/api/v1/objects/${encodeURIComponent(id)}`)),
  track: (id, afterMin, stepS) =>
    getJson(withView(`/api/v1/objects/${encodeURIComponent(id)}/track?after_min=${afterMin}&step_s=${stepS}`)),
  search: (q) => {
    // 搜索整个编目,不受当前过滤条件限制
    const p = new URLSearchParams({
      q, per_page: '12', alt_min: '-1000', alt_max: '1000000', min_size_cm: '0', any_age: 'true', types: '',
    });
    return getJson(withView(`/api/v1/objects?${p}`));
  },
  settings: () => getJson('/api/v1/settings'),
  putSettings: (body) =>
    getJson('/api/v1/settings', {
      method: 'PUT',
      headers: writeHeaders(),
      body: JSON.stringify(body),
    }),
  stats: (queryString, binKm) => getJson(withView(`/api/v1/stats?${queryString}&bin_km=${binKm}`)),
};
