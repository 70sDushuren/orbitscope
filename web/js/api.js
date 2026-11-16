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
    throw new Error((body && body.error) || `HTTP ${resp.status}`);
  }
  return body;
}

export const api = {
  clientConfig: () => getJson('/api/v1/client-config'),
  status: () => getJson('/api/v1/status'),
  defaultFilter: () => getJson('/api/v1/filters/default'),
  object: (id) => getJson(`/api/v1/objects/${encodeURIComponent(id)}`),
  track: (id, afterMin, stepS) =>
    getJson(`/api/v1/objects/${encodeURIComponent(id)}/track?after_min=${afterMin}&step_s=${stepS}`),
  search: (q) => {
    // 搜索整个编目,不受当前过滤条件限制
    const p = new URLSearchParams({
      q, per_page: '12', alt_min: '-1000', alt_max: '1000000', min_size_cm: '0', any_age: 'true', types: '',
    });
    return getJson(`/api/v1/objects?${p}`);
  },
  stats: (queryString, binKm) => getJson(`/api/v1/stats?${queryString}&bin_km=${binKm}`),
};
