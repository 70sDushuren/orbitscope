// 前端冒烟测试(Playwright + Chromium)。
//
// 一步运行(构建好二进制后):scripts/smoke.sh(CI 也用它;自动准备 testdata、启动服务并运行本脚本)。
// 或手动:先启动服务(用 testdata 与 --sim-time 2024-01-01T12:30:00Z),再运行:
//   node scripts/browser-smoke.js http://127.0.0.1:8080 /tmp/shots
// 需要可 require('playwright') 的环境。WebGL 使用 SwiftShader 软件渲染,帧率不代表真实 GPU。
const { chromium } = require('playwright');

const BASE = process.argv[2] || 'http://127.0.0.1:8080';
const OUT = process.argv[3] || '.';

(async () => {
  const browser = await chromium.launch({ args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'] });
  // 固定为简体中文,便于对照截图;语言切换另有 SMOKE_LOCALE 控制
  const page = await browser.newPage({ viewport: { width: 1400, height: 860 }, deviceScaleFactor: 1, locale: process.env.SMOKE_LOCALE || 'zh-CN' });
  const errors = [];
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  const fail = (msg) => { console.error('FAIL:', msg); process.exitCode = 1; };

  await page.goto(`${BASE}/`);
  await page.waitForFunction(() => document.getElementById('loading').hidden, null, { timeout: 90000 });
  await page.waitForFunction(() => window.orbitscope && window.orbitscope.state.lastFrameMs !== undefined, null, { timeout: 30000 });
  const info = await page.evaluate(() => ({
    shown: document.getElementById('st-shown').textContent,
    conn: window.orbitscope.state.connected,
    lang: document.documentElement.lang,
    stats: document.getElementById('stats-total').textContent,
    frameApplyMs: window.orbitscope.state.lastFrameMs,
    catalogMs: window.orbitscope.state.lastCatalogMs,
  }));
  console.log(JSON.stringify(info));
  if (info.conn !== true) fail('WebSocket 未连接');
  await page.screenshot({ path: `${OUT}/overview.png` });

  // 搜索第一个结果并打开详情
  await page.fill('#search', process.env.SMOKE_QUERY || 'ISS');
  await page.waitForSelector('#search-results li button', { timeout: 10000 });
  await page.click('#search-results li button');
  await page.waitForFunction(() => !document.getElementById('right').hidden, null, { timeout: 10000 });
  await page.waitForTimeout(2000);
  console.log('详情:', await page.textContent('#d-name'), await page.textContent('#d-alt'));
  await page.screenshot({ path: `${OUT}/details.png` });

  // "指定时刻"只对本页面生效:切到过去的时刻,时钟从该时刻继续走,再回到实时
  const liveTime = await page.textContent('#st-time');
  await page.check('input[name=time_mode][value=fixed]');
  await page.evaluate(() => {
    const i = document.querySelector('input[name=fixed_time]');
    i.value = '2024-01-01T12:15:00';
    i.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.click('#settings-form button[type=submit]');
  await page.waitForFunction(() => window.orbitscope.state.timeMode === 'fixed' && document.getElementById('st-time').textContent.includes('12:15:'), null, { timeout: 15000 })
    .catch(() => fail('未能切换到指定时刻'));
  if (await page.evaluate(() => document.getElementById('st-replay').hidden)) fail('指定时刻下未显示回放标记');
  if (await page.textContent('#settings-error')) fail(`切换时刻不应报错: ${await page.textContent('#settings-error')}`);
  await page.check('input[name=time_mode][value=live]');
  await page.click('#settings-form button[type=submit]');
  // 回到实时后应直接显示服务端时刻(12:3x),不能出现两个时刻之间的插值
  await page.waitForFunction(() => window.orbitscope.state.timeMode === 'live' && document.getElementById('st-time').textContent.includes(' 12:3'), null, { timeout: 15000 })
    .catch(() => fail('未能回到实时'));
  // 已选中物体的实时经纬度在切换后仍能显示
  await page.waitForFunction(() => /\d/.test(document.getElementById('d-latlon').textContent), null, { timeout: 15000 })
    .catch(() => fail('切换时间后详情面板没有实时位置'));
  console.log('时间切换:', liveTime, '→ 指定时刻 → ', await page.textContent('#st-time'));

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
