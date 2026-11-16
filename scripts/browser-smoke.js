// 前端冒烟测试(Playwright + Chromium)。
//
// 用法:先启动服务(建议用 testdata 与 --sim-time),再运行:
//   node scripts/browser-smoke.js http://127.0.0.1:8080 /tmp/shots
// 需要可 require('playwright') 的环境。WebGL 使用 SwiftShader 软件渲染,帧率不代表真实 GPU。
const { chromium } = require('playwright');

const BASE = process.argv[2] || 'http://127.0.0.1:8080';
const OUT = process.argv[3] || '.';

(async () => {
  const browser = await chromium.launch({ args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'] });
  const page = await browser.newPage({ viewport: { width: 1400, height: 860 }, deviceScaleFactor: 1 });
  const errors = [];
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  const fail = (msg) => { console.error('FAIL:', msg); process.exitCode = 1; };

  await page.goto(`${BASE}/`);
  await page.waitForFunction(() => document.getElementById('loading').hidden, null, { timeout: 90000 });
  await page.waitForFunction(() => window.orbitscope && window.orbitscope.state.lastFrameMs !== undefined, null, { timeout: 30000 });
  const info = await page.evaluate(() => ({
    shown: document.getElementById('st-shown').textContent,
    conn: document.getElementById('st-conn').textContent,
    stats: document.getElementById('stats-total').textContent,
    frameApplyMs: window.orbitscope.state.lastFrameMs,
    catalogMs: window.orbitscope.state.lastCatalogMs,
  }));
  console.log(JSON.stringify(info));
  if (info.conn !== '已连接') fail('WebSocket 未连接');
  await page.screenshot({ path: `${OUT}/overview.png` });

  // 搜索第一个结果并打开详情
  await page.fill('#search', process.env.SMOKE_QUERY || 'ISS');
  await page.waitForSelector('#search-results li button', { timeout: 10000 });
  await page.click('#search-results li button');
  await page.waitForFunction(() => !document.getElementById('right').hidden, null, { timeout: 10000 });
  await page.waitForTimeout(2000);
  console.log('详情:', await page.textContent('#d-name'), await page.textContent('#d-alt'));
  await page.screenshot({ path: `${OUT}/details.png` });

  // 非法过滤条件应在表单内提示
  await page.fill('input[name=alt_min_km]', '5000');
  await page.click('#filter-form button[type=submit]');
  if (!(await page.textContent('#filter-error'))) fail('非法过滤条件未提示');

  // 手机宽度无横向滚动
  await page.setViewportSize({ width: 390, height: 800 });
  await page.waitForTimeout(500);
  if (await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)) fail('手机宽度出现横向滚动');
  await page.screenshot({ path: `${OUT}/mobile.png` });

  if (errors.length) fail(`控制台错误:\n${errors.join('\n')}`);
  await browser.close();
  console.log(process.exitCode ? '有失败项' : '全部通过');
})();
