#!/usr/bin/env node
// Защита i18n (этап 11.2): пользовательские строки живут только в каталогах
// src/lib/i18n/*.json. Скрипт ищет литералы с кириллицей/CJK вне комментариев:
//   - src/**/*.{svelte,ts} (кроме src/lib/i18n/);
//   - src-tauri/src/**/*.rs (кроме блоков #[cfg(test)] — тестовые фикстуры легитимны).
// Формат вывода: файл:строка: фрагмент; код возврата 1 при непустом списке.
// Полноценная CI-проводка — этап D3 плана 0.2. Чистый node, без зависимостей.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const ROOT = new URL('..', import.meta.url).pathname;

// Кириллица + CJK (хань, хирагана, катакана) — по ТЗ этапа 11.
const BAD = /[\u0400-\u04FF\u4E00-\u9FFF\u3040-\u30FF]/;

function* walk(dir, match) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) yield* walk(p, match);
    else if (match(name)) yield p;
  }
}

function collectFiles() {
  const out = [];
  for (const p of walk(join(ROOT, 'src'), (n) => /\.(svelte|ts)$/.test(n))) {
    const rel = relative(ROOT, p).split(/[\\/]+/).join('/');
    if (rel.startsWith('src/lib/i18n/')) continue; // каталог — исключение
    out.push(rel);
  }
  for (const p of walk(join(ROOT, 'src-tauri', 'src'), (n) => n.endsWith('.rs'))) {
    out.push(relative(ROOT, p).split(/[\\/]+/).join('/'));
  }
  return out.sort();
}

const countNewlines = (s) => s.split('\n').length - 1;

// --- TS/JS: строки, шаблонные литералы с ${}, комментарии // и /* */ ---
function scanTs(src, lineOffset) {
  const hits = new Set();
  let line = 1 + lineOffset;
  let i = 0;
  const n = src.length;
  let state = 'code'; // code | line | block | sq | dq | tpl
  const tplBraceStack = []; // счётчики вложенных { внутри ${...}
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === '\n') {
      // строки в кавычках не многострочные — перенос закрывает их (защита от рассинхрона)
      if (state === 'line' || state === 'sq' || state === 'dq') state = 'code';
      line++;
      i++;
      continue;
    }
    if (state === 'code') {
      if (c === '/' && d === '/') { state = 'line'; i += 2; continue; }
      if (c === '/' && d === '*') { state = 'block'; i += 2; continue; }
      if (c === "'") { state = 'sq'; i++; continue; }
      if (c === '"') { state = 'dq'; i++; continue; }
      if (c === '`') { state = 'tpl'; i++; continue; }
      if (tplBraceStack.length && (c === '{' || c === '}')) {
        const top = tplBraceStack.length - 1;
        if (c === '{') tplBraceStack[top]++;
        else if (tplBraceStack[top] === 0) {
          tplBraceStack.pop();
          state = 'tpl';
        } else tplBraceStack[top]--;
      }
      if (BAD.test(c)) hits.add(line);
      i++;
      continue;
    }
    if (state === 'line') {
      i++; // до \n
      continue;
    }
    if (state === 'block') {
      if (c === '*' && d === '/') { state = 'code'; i += 2; } else i++;
      continue;
    }
    if (state === 'sq' || state === 'dq') {
      if (c === '\\') {
        if (d === '\n') line++;
        i += 2;
        continue;
      }
      if (c === (state === 'sq' ? "'" : '"')) { state = 'code'; i++; continue; }
      if (BAD.test(c)) hits.add(line);
      i++;
      continue;
    }
    // tpl
    if (c === '\\') { i += 2; continue; }
    if (c === '$' && d === '{') { tplBraceStack.push(0); state = 'code'; i += 2; continue; }
    if (c === '`') { state = 'code'; i++; continue; }
    if (BAD.test(c)) hits.add(line);
    i++;
  }
  return hits;
}

// --- Rust: /// //! // /* */ (вложенные), "", '' vs лайфтаймы, r#""#; пропуск #[cfg(test)] ---
// Пропуск вызовов log::…!(...): логи — диагностика, а не пользовательские строки
// (решение этапа 11: не переводятся в каталог, обоснование — в отчёте).
function skipLogCall(src, i) {
  if (i > 0 && /[\w:]/.test(src[i - 1])) return -1;
  const m = /^log::(?:info|warn|error|debug|trace)!\s*\(/.exec(src.slice(i, i + 48));
  if (!m) return -1;
  let k = i + m[0].length;
  let depth = 1;
  while (k < src.length && depth > 0) {
    const c = src[k];
    const d = src[k + 1];
    if (c === '/' && d === '/') {
      const nl = src.indexOf('\n', k);
      k = nl === -1 ? src.length : nl;
      continue;
    }
    if (c === '/' && d === '*') {
      let bd = 1;
      k += 2;
      while (k < src.length && bd > 0) {
        if (src[k] === '/' && src[k + 1] === '*') { bd++; k += 2; }
        else if (src[k] === '*' && src[k + 1] === '/') { bd--; k += 2; }
        else k++;
      }
      continue;
    }
    if (c === '(') { depth++; k++; continue; }
    if (c === ')') { depth--; k++; continue; }
    if (c === '"') {
      k++;
      while (k < src.length) {
        if (src[k] === '\\') { k += 2; continue; }
        if (src[k] === '"') { k++; break; }
        k++;
      }
      continue;
    }
    if (c === 'r' || c === 'b' || c === 'c') {
      let j = c === 'r' ? k : k + 1;
      if (src[j] === 'r') {
        j++;
        let h = 0;
        while (src[j] === '#') { h++; j++; }
        if (src[j] === '"') {
          const term = '"' + '#'.repeat(h);
          const end = src.indexOf(term, j + 1);
          k = end === -1 ? src.length : end + term.length;
          continue;
        }
      }
    }
    if (c === String.fromCharCode(39)) {
      if (src[k + 1] === '\\') {
        // '\x' / '\u{..}' — до закрывающей '
        const close = src.indexOf(String.fromCharCode(39), k + 3);
        k = close === -1 ? src.length : close + 1;
        continue;
      }
      if (src[k + 2] === String.fromCharCode(39)) { k += 3; continue; }
    }
    k++;
  }
  return k;
}

function scanRust(src) {
  const hits = new Set();
  let line = 1;
  let i = 0;
  const n = src.length;
  let state = 'code'; // code | line | block | str | char | raw
  let blockDepth = 0;
  let rawEnd = '';
  let inTest = false;
  let testDepth = 0;
  let cfgTestPending = false;
  const mark = () => {
    if (!inTest) hits.add(line);
  };
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === '\n') {
      // перенос закрывает line/str/char (raw-строки многострочные — не трогаем)
      if (state === 'line' || state === 'str' || state === 'char') state = 'code';
      line++;
      i++;
      continue;
    }
    if (state === 'code') {
      if (c === '/' && d === '/') { state = 'line'; i += 2; continue; }
      if (c === '/' && d === '*') { state = 'block'; blockDepth = 1; i += 2; continue; }
      if (c === 'l') {
        const logEnd = skipLogCall(src, i);
        if (logEnd !== -1) {
          line += countNewlines(src.slice(i, logEnd));
          i = logEnd;
          continue;
        }
      }
      if (c === '#' && src.startsWith('#[cfg(test)]', i)) {
        cfgTestPending = true;
        i += '#[cfg(test)]'.length;
        continue;
      }
      if (c === '{' && cfgTestPending) {
        cfgTestPending = false;
        inTest = true;
        testDepth = 1;
        i++;
        continue;
      }
      if (inTest) {
        if (c === '{') testDepth++;
        else if (c === '}') {
          testDepth--;
          if (testDepth === 0) inTest = false;
        }
      }
      if (c === '"') { state = 'str'; i++; continue; }
      if (c === 'r' || c === 'b' || c === 'c') {
        // префиксы raw-строк: r" r#" br##" c" ... (ключ — наличие r перед #*")
        let k = i;
        if (c === 'b' || c === 'c') k++;
        if (src[k] === 'r') {
          let j = k + 1;
          let hashes = 0;
          while (src[j] === '#') { hashes++; j++; }
          if (src[j] === '"') {
            rawEnd = '"' + '#'.repeat(hashes);
            state = 'raw';
            i = j + 1;
            continue;
          }
        }
      }
      if (c === "'") {
        // char-литерал: 'x' или '\x'; иначе лайфтайм ('a, 'static) — остаёмся в code
        if (d === '\\') { state = 'char'; i += 2; continue; }
        if (src[i + 2] === "'") { state = 'char'; i++; continue; }
        if (BAD.test(c)) mark();
        i++;
        continue;
      }
      if (BAD.test(c)) mark();
      i++;
      continue;
    }
    if (state === 'line') {
      i++;
      continue;
    }
    if (state === 'block') {
      if (c === '/' && d === '*') { blockDepth++; i += 2; continue; }
      if (c === '*' && d === '/') {
        blockDepth--;
        if (blockDepth === 0) state = 'code';
        i += 2;
        continue;
      }
      i++;
      continue;
    }
    if (state === 'str') {
      if (c === '\\') {
        if (d === '\n') line++;
        i += 2;
        continue;
      }
      if (c === '"') { state = 'code'; i++; continue; }
      if (BAD.test(c)) mark();
      i++;
      continue;
    }
    if (state === 'char') {
      if (c === '\\') { i += 2; continue; }
      if (c === "'") { state = 'code'; i++; continue; }
      if (BAD.test(c)) mark();
      i++;
      continue;
    }
    // raw
    if (c === '"' && src.startsWith(rawEnd, i)) {
      state = 'code';
      i += rawEnd.length;
      continue;
    }
    if (BAD.test(c)) mark();
    i++;
  }
  return hits;
}

// --- Svelte: HTML-комментарии, {/* ... */}, <script>(TS), <style>(CSS), текст шаблона ---
function scanSvelte(src) {
  const hits = new Set();
  let line = 1;
  let i = 0;
  const n = src.length;
  let ctx = 'html'; // html | css
  let cssComment = false;
  while (i < n) {
    const c = src[i];
    if (c === '\n') {
      line++;
      i++;
      continue;
    }
    if (ctx === 'html') {
      if (src.startsWith('<!--', i)) {
        const end = src.indexOf('-->', i + 4);
        const stop = end === -1 ? n : end + 3;
        line += countNewlines(src.slice(i, stop));
        i = stop;
        continue;
      }
      if (src.startsWith('{/*', i)) {
        const end = src.indexOf('*/}', i + 3);
        const stop = end === -1 ? n : end + 3;
        line += countNewlines(src.slice(i, stop));
        i = stop;
        continue;
      }
      if (c === '<' && /^<script[\s>/]/i.test(src.slice(i, i + 20))) {
        const openEnd = src.indexOf('>', i);
        const close = src.indexOf('</script', openEnd);
        const bodyStart = openEnd + 1;
        const bodyEnd = close === -1 ? n : close;
        for (const h of scanTs(src.slice(bodyStart, bodyEnd), countNewlines(src.slice(0, bodyStart)))) {
          hits.add(h);
        }
        line += countNewlines(src.slice(i, bodyEnd));
        i = bodyEnd;
        continue;
      }
      if (c === '<' && /^<style[\s>/]/i.test(src.slice(i, i + 20))) {
        ctx = 'css';
        i++;
        continue;
      }
      if (BAD.test(c)) hits.add(line);
      i++;
      continue;
    }
    // css
    if (src.startsWith('</style', i)) {
      ctx = 'html';
      i++;
      continue;
    }
    if (cssComment) {
      if (src.startsWith('*/', i)) {
        cssComment = false;
        i += 2;
      } else i++;
      continue;
    }
    if (src.startsWith('/*', i)) {
      cssComment = true;
      i += 2;
      continue;
    }
    if (BAD.test(c)) hits.add(line);
    i++;
  }
  return hits;
}

export { scanTs, scanRust, scanSvelte, collectFiles };

const isMain = process.argv[1] && import.meta.url.endsWith(process.argv[1].split('/').pop());
if (isMain) main();

function main() {
const violations = [];
const files = collectFiles();
for (const rel of files) {
  const src = readFileSync(join(ROOT, rel), 'utf8');
  let hits;
  if (rel.endsWith('.rs')) hits = scanRust(src);
  else if (rel.endsWith('.svelte')) hits = scanSvelte(src);
  else hits = scanTs(src, 0);
  if (hits.size === 0) continue;
  const lines = src.split('\n');
  for (const ln of [...hits].sort((a, b) => a - b)) {
    const text = (lines[ln - 1] ?? '').trim();
    violations.push(`${rel}:${ln}: ${text.length > 90 ? text.slice(0, 90) + '…' : text}`);
  }
}

if (violations.length) {
  console.error(`i18n-check: нарушений: ${violations.length}`);
  for (const v of violations) console.error(`  ${v}`);
  console.error('Пользовательские строки — только в src/lib/i18n/*.json (TODO.md, этап 11).');
  process.exit(1);
}
console.log(`i18n-check: OK, файлов проверено: ${files.length}, нарушений нет.`);
}
