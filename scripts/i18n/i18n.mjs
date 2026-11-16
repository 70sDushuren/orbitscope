#!/usr/bin/env node
// OrbitScope 界面多语言工具。
//
// 源语言为简体中文(web/i18n/zh-CN.json),目标语言见 TARGETS。每条译文对应的源文字指纹记录在
// scripts/i18n/translation-state.json 中,源文字改动后该条译文即视为"过期"。
//
// 用法(在仓库根目录):
//   node scripts/i18n/i18n.mjs check [--warn]      检查缺失 / 多余 / 占位符不一致 / 过期;有问题时退出码 1(--warn 只警告)
//   node scripts/i18n/i18n.mjs translate [--dry-run] [--locale en]
//                                                  用 Claude API 补译缺失与过期条目(需要 ANTHROPIC_API_KEY)
//   node scripts/i18n/i18n.mjs stamp [--locale en] 人工翻译后,把现有译文标记为与当前源文字一致
//
// 发布流程(.github/workflows/release-prepare.yml)会先运行 translate,再运行 check。

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(here, '../..');
const I18N_DIR = path.join(ROOT, 'web/i18n');
const STATE_FILE = path.join(here, 'translation-state.json');
const SOURCE = 'zh-CN';
const TARGETS = ['en', 'zh-HK'];
const MODEL = 'claude-opus-5-5';
const BATCH = 60;

/** 各目标语言的翻译要求(写进系统提示)。 */
const STYLE = {
  en: [
    'Target: English for a technical web UI (space-object tracking).',
    'Use concise, natural UI English in sentence case. Keep labels short.',
  ].join('\n'),
  'zh-HK': [
    '目標語言:香港繁體中文,用香港粵語書面語(口語化),適合網頁介面。',
    '用粵語字詞,例如「嘅」「唔」「冇」「揀」「喺」「咗」「緊」;用香港常用詞,例如「搜尋」「資料」「設定」「伺服器」「快取」「酬載」。',
    '保持簡潔,介面標籤要短。',
  ].join('\n'),
};

const COMMON_RULES = [
  'You translate user-interface strings for OrbitScope, a near-real-time tracker for objects in Earth orbit.',
  'The source language is Simplified Chinese.',
  'Rules:',
  '- Keep every placeholder in curly braces exactly as written, e.g. {n}, {time}, {error}. Do not translate or drop them.',
  '- Keep product, command and technical names unchanged: OrbitScope, orbitscope collect, orbitscope fetch, CesiumJS, Cesium,',
  '  server.cesium_dir, deploy/, NORAD, RCS, UTC, km, m, Web, US, PRC, CIS.',
  '- Keep symbols such as ≈, <, >, ≤, –, °, … and the meaning of numbers.',
  '- Match the terminology of the existing translations given as reference.',
  '- Return one translation for every key you are given, and only those keys.',
].join('\n');

// ------------------------------------------------------------------ 文件读写

function readJson(file, fallback) {
  if (!fs.existsSync(file)) return fallback;
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function writeJson(file, obj) {
  fs.writeFileSync(file, `${JSON.stringify(obj, null, 2)}\n`);
}

const localeFile = (id) => path.join(I18N_DIR, `${id}.json`);
const hash = (text) => createHash('sha256').update(text).digest('hex').slice(0, 16);
const placeholders = (text) => [...String(text).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');

function loadAll() {
  const source = readJson(localeFile(SOURCE), null);
  if (!source) throw new Error(`缺少源语言文件 ${localeFile(SOURCE)}`);
  const targets = Object.fromEntries(TARGETS.map((id) => [id, readJson(localeFile(id), {})]));
  const state = readJson(STATE_FILE, {});
  return { source, targets, state };
}

/** 按源文件的键顺序写出目标文件(去掉源中已不存在的键)。 */
function saveTarget(id, source, dict) {
  const ordered = {};
  for (const key of Object.keys(source)) if (key in dict) ordered[key] = dict[key];
  writeJson(localeFile(id), ordered);
}

// ------------------------------------------------------------------ 检查

/** 返回某目标语言需要(重新)翻译的键,以及其他问题。 */
function audit(id, source, dict, state) {
  const record = state[id] || {};
  const todo = [];
  const problems = [];
  for (const [key, text] of Object.entries(source)) {
    if (!(key in dict)) {
      todo.push(key);
      problems.push(`缺失  ${key}`);
    } else if (placeholders(dict[key]) !== placeholders(text)) {
      todo.push(key);
      problems.push(`占位符不一致  ${key}:源 {${placeholders(text)}},译文 {${placeholders(dict[key])}}`);
    } else if (!(key in record)) {
      todo.push(key);
      problems.push(`未确认(没有翻译记录)  ${key}`);
    } else if (record[key] !== hash(text)) {
      todo.push(key);
      problems.push(`过期(源文字已改动)  ${key}`);
    }
  }
  for (const key of Object.keys(dict)) if (!(key in source)) problems.push(`多余(源中已删除)  ${key}`);
  return { todo, problems };
}

function cmdCheck({ warn }) {
  const { source, targets, state } = loadAll();
  let total = 0;
  for (const id of TARGETS) {
    const { problems } = audit(id, source, targets[id], state);
    total += problems.length;
    if (problems.length === 0) {
      console.log(`✓ ${id}:${Object.keys(source).length} 条,全部最新`);
      continue;
    }
    console.log(`✗ ${id}:${problems.length} 个问题`);
    for (const p of problems) {
      console.log(`    ${p}`);
      if (process.env.GITHUB_ACTIONS) console.log(`::${warn ? 'warning' : 'error'} file=web/i18n/${id}.json::${p}`);
    }
  }
  if (total > 0 && !warn) {
    console.log('\n运行 `node scripts/i18n/i18n.mjs translate` 自动补译,或人工翻译后运行 `stamp`。');
    process.exitCode = 1;
  }
}

function cmdStamp({ locales }) {
  const { source, targets, state } = loadAll();
  for (const id of locales) {
    const record = {};
    for (const [key, text] of Object.entries(source)) {
      if (key in targets[id] && placeholders(targets[id][key]) === placeholders(text)) record[key] = hash(text);
    }
    state[id] = record;
    saveTarget(id, source, targets[id]);
    console.log(`已标记 ${id}:${Object.keys(record).length} / ${Object.keys(source).length} 条`);
  }
  writeJson(STATE_FILE, state);
}

// ------------------------------------------------------------------ 翻译

/** 调用 Claude 翻译一批条目,返回 { key: text }。 */
async function translateBatch(client, id, items, reference) {
  const { default: Anthropic } = await import('@anthropic-ai/sdk');
  const { z } = await import('zod');
  const { betaZodOutputFormat } = await import('@anthropic-ai/sdk/helpers/beta/zod');
  const Schema = z.object({
    translations: z.array(z.object({ key: z.string(), text: z.string() })),
  });
  const payload = {
    target_locale: id,
    reference_translations: reference,
    items: items.map(([key, text]) => ({ key, source: text })),
  };
  let response;
  try {
    response = await client.beta.messages.parse({
      model: MODEL,
      max_tokens: 16000,
      // 被安全分类器拒绝时由服务端自动改用备用模型
      betas: ['server-side-fallback-2026-07-01'],
      fallbacks: 'default',
      output_config: { effort: 'medium', format: betaZodOutputFormat(Schema) },
      system: `${COMMON_RULES}\n\n${STYLE[id]}`,
      messages: [{ role: 'user', content: JSON.stringify(payload, null, 1) }],
    });
  } catch (e) {
    if (e instanceof Anthropic.AuthenticationError) throw new Error('Claude API 认证失败:请检查 ANTHROPIC_API_KEY');
    if (e instanceof Anthropic.RateLimitError) throw new Error('Claude API 限流,请稍后重试');
    if (e instanceof Anthropic.APIError) throw new Error(`Claude API 错误 ${e.status}: ${e.message}`);
    throw e;
  }
  if (response.stop_reason === 'refusal') throw new Error(`翻译请求被拒绝(${response.stop_details?.category ?? '未知类别'})`);
  if (response.stop_reason === 'max_tokens') throw new Error('输出被截断(max_tokens),请减小批量');
  const parsed = response.parsed_output;
  if (!parsed) throw new Error('无法解析模型输出');
  const wanted = new Map(items);
  const out = {};
  for (const { key, text } of parsed.translations) {
    if (!wanted.has(key)) continue;
    if (placeholders(text) !== placeholders(wanted.get(key))) {
      console.warn(`  ! ${key}:占位符不一致,跳过(将在 check 中报告)`);
      continue;
    }
    out[key] = text;
  }
  return out;
}

/** 测试用的离线"翻译器":在源文字前加语言标记,保留占位符。 */
function fakeTranslate(id, items) {
  return Object.fromEntries(items.map(([key, text]) => [key, `[${id}] ${text}`]));
}

async function cmdTranslate({ locales, dryRun, fake }) {
  const { source, targets, state } = loadAll();
  const plan = locales.map((id) => [id, audit(id, source, targets[id], state).todo]);
  for (const [id, todo] of plan) console.log(`${id}:需要翻译 ${todo.length} 条${todo.length ? `(${todo.join(', ')})` : ''}`);
  if (dryRun || plan.every(([, todo]) => todo.length === 0)) return;

  let client = null;
  if (!fake) {
    if (!process.env.ANTHROPIC_API_KEY && !process.env.ANTHROPIC_AUTH_TOKEN) {
      throw new Error('有待翻译的条目,但未设置 ANTHROPIC_API_KEY(在 GitHub 仓库 Settings → Secrets 中添加)');
    }
    const { default: Anthropic } = await import('@anthropic-ai/sdk');
    client = new Anthropic();
  }

  for (const [id, todo] of plan) {
    if (todo.length === 0) continue;
    const dict = { ...targets[id] };
    const record = { ...(state[id] || {}) };
    // 现有的最新译文作为术语参考
    const reference = Object.fromEntries(
      Object.entries(dict)
        .filter(([k]) => k in source && !todo.includes(k))
        .map(([k, v]) => [k, { source: source[k], translation: v }]),
    );
    for (let i = 0; i < todo.length; i += BATCH) {
      const items = todo.slice(i, i + BATCH).map((k) => [k, source[k]]);
      const result = fake ? fakeTranslate(id, items) : await translateBatch(client, id, items, reference);
      for (const [key, text] of Object.entries(result)) {
        dict[key] = text;
        record[key] = hash(source[key]);
      }
      console.log(`  ${id}:已翻译 ${Object.keys(result).length} / ${items.length} 条`);
    }
    // 删除源中已不存在的键与记录
    for (const key of Object.keys(record)) if (!(key in source)) delete record[key];
    state[id] = record;
    saveTarget(id, source, dict);
  }
  writeJson(STATE_FILE, state);
}

// ------------------------------------------------------------------ 入口

const [cmd, ...rest] = process.argv.slice(2);
const flag = (name) => rest.includes(name);
const localeArg = rest.includes('--locale') ? rest[rest.indexOf('--locale') + 1] : null;
const locales = localeArg ? [localeArg] : TARGETS;
if (localeArg && !TARGETS.includes(localeArg)) {
  console.error(`未知语言 ${localeArg}(可选 ${TARGETS.join(', ')})`);
  process.exit(2);
}

try {
  switch (cmd) {
    case 'check':
      cmdCheck({ warn: flag('--warn') });
      break;
    case 'stamp':
      cmdStamp({ locales });
      break;
    case 'translate':
      await cmdTranslate({ locales, dryRun: flag('--dry-run'), fake: flag('--fake') });
      break;
    default:
      console.error('用法:i18n.mjs check [--warn] | translate [--dry-run] [--locale ID] | stamp [--locale ID]');
      process.exit(2);
  }
} catch (e) {
  console.error(`错误:${e.message}`);
  process.exit(1);
}
