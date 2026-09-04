<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { getVersion } from '@tauri-apps/api/app';
  import HotkeyInput from '$lib/components/HotkeyInput.svelte';
  import MicTest from '$lib/components/MicTest.svelte';
  import { validateHotkey, OS_DEFAULT_HOTKEY } from '$lib/hotkey';
  import { t, formatInt, formatDecimal, tPlural } from '$lib/i18n/index.svelte';

  type Provider = 'openai' | 'qwen' | 'google';
  type Theme = 'system' | 'light' | 'dark';
  type Section = 'connect' | 'input' | 'autostart' | 'check' | 'stats';

  type Settings = {
    provider: Provider;
    endpoint: string;
    token: string;
    model: string;
    language: string;
    hotkey: string;
    insertDelayMs: number;
    autostart: boolean;
    theme: Theme;
    inputDevice: string;
    typingSpeedWpm: number;
    statsEnabled: boolean;
    locale: string;
  };

  type InputDevice = { name: string; isDefault: boolean; formats: string[] };

  type DayStats = { words: number; chars: number; sessions: number; audioSec: number };
  type ChartPoint = { date: string; words: number };
  type StatsSummary = {
    today: DayStats;
    weekWords: number;
    lifetime: DayStats;
    streakDays: number;
    bestDayWords: number;
    minutesSavedToday: number;
    minutesSavedTotal: number;
    chart: ChartPoint[];
  };

  const QWEN_DEFAULT_ENDPOINT =
    'https://dashscope-intl.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation';
  const OPENAI_DEFAULT_ENDPOINT = 'https://api.openai.com/v1';
  const QWEN_DEFAULT_MODEL = 'qwen-audio-3.0-asr-flash';
  const OPENAI_DEFAULT_MODEL = 'whisper-1';

  // Имена языков диктовки — ISO-названия, не переводятся (кроме «авто»);
  // лежат в каталоге, чтобы в svelte-файлах не было литералов с кириллицей.
  const LANGUAGE_CODES = ['', 'ru', 'en', 'es', 'fr', 'de', 'it', 'pt', 'pl', 'tr', 'uk', 'zh', 'ja'];

  const SECTIONS: [Section, string][] = [
    ['connect', '🔌'],
    ['input', '⌨️'],
    ['autostart', '🚀'],
    ['check', '🎙'],
    ['stats', '📈'],
  ];

  const LOCALES: [string, string | null][] = [
    ['auto', 'ui.locale.auto'],
    ['en', null],
    ['ru', null],
    ['zh', null],
  ];

  let settings = $state<Settings>({
    provider: 'openai',
    endpoint: '',
    token: '',
    model: 'whisper-1',
    language: '',
    hotkey: 'Cmd+Shift+Space',
    insertDelayMs: 50,
    autostart: false,
    theme: 'system',
    inputDevice: '',
    typingSpeedWpm: 40,
    statsEnabled: true,
    locale: 'auto',
  });
  let devices = $state<InputDevice[]>([]);
  let devicesError = $state('');
  let appVersion = $state('');
  getVersion().then((v) => (appVersion = v)).catch(() => {});

  function loadDevices() {
    invoke<InputDevice[]>('list_input_devices')
      .then((d) => {
        devices = d;
        devicesError = '';
      })
      .catch((e) => (devicesError = String(e)));
  }
  loadDevices();
  let loaded = $state(false);
  let loadingError = $state('');
  let saveError = $state('');
  let showToken = $state(false);
  let saved = $state(false);
  let checking = $state(false);
  let checkResult = $state<TestResult | null>(null);
  let active = $state<Section>('connect');
  let advOpen = $state(false);

  type TestResult = { ok: boolean; text: string | null; latencyMs: number; error: string | null };

  let stats = $state<StatsSummary | null>(null);
  let statsError = $state('');

  function loadStats() {
    if (!settings.statsEnabled) return;
    invoke<StatsSummary>('get_stats')
      .then((s) => {
        stats = s;
        statsError = '';
      })
      .catch((e) => (statsError = String(e)));
  }

  function fmtMinutes(m: number): string {
    const min = t('ui.time.min');
    if (m < 60) return `${formatInt(m)} ${min}`;
    const h = Math.floor(m / 60);
    const mm = m % 60;
    return mm ? `${formatInt(h)} ${t('ui.time.hour')} ${formatInt(mm)} ${min}` : `${formatInt(h)} ${t('ui.time.hour')}`;
  }

  let chartMax = $derived(stats ? Math.max(1, ...stats.chart.map((p) => p.words)) : 1);

  async function testConnection() {
    checking = true;
    checkResult = null;
    try {
      checkResult = await invoke<TestResult>('test_connection');
    } catch (e) {
      checkResult = { ok: false, text: null, latencyMs: 0, error: String(e) };
    } finally {
      checking = false;
    }
  }

  function onProviderChange(next: Provider) {
    settings.provider = next;
    const ep = settings.endpoint.trim();
    const model = settings.model.trim();
    if (next === 'qwen') {
      if (ep === '' || ep === OPENAI_DEFAULT_ENDPOINT) {
        settings.endpoint = QWEN_DEFAULT_ENDPOINT;
      }
      if (model === '' || model === OPENAI_DEFAULT_MODEL) {
        settings.model = QWEN_DEFAULT_MODEL;
      }
    } else if (next === 'google') {
      // Google: эндпоинт/токен/модель не используются — очищаем только
      // значения-дефолты других провайдеров, свои данные пользователя не трогаем.
      if (ep === '' || ep === OPENAI_DEFAULT_ENDPOINT || ep === QWEN_DEFAULT_ENDPOINT) {
        settings.endpoint = '';
      }
      if (model === '' || model === OPENAI_DEFAULT_MODEL || model === QWEN_DEFAULT_MODEL) {
        settings.model = '';
      }
    } else {
      if (ep === QWEN_DEFAULT_ENDPOINT) {
        settings.endpoint = '';
      }
      if (model === '' || model === QWEN_DEFAULT_MODEL) {
        settings.model = OPENAI_DEFAULT_MODEL;
      }
    }
  }

  let endpointError = $derived(validateEndpoint(settings.endpoint));
  let hotkeyError = $derived(validateHotkey(settings.hotkey));
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  // Возвращает ключ каталога ('' — ОК), текст — t(key) в шаблоне.
  function validateEndpoint(value: string): string {
    const v = value.trim();
    if (!v) return '';
    let url: URL;
    try {
      url = new URL(v);
    } catch {
      return 'ui.validation.endpoint_url';
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      return 'ui.validation.endpoint_protocol';
    }
    return '';
  }

  // «Дополнительно» само раскрывается, если значения не совпадают с пресетом
  // провайдера или эндпоинт невалиден (закрыть вручную при этом можно).
  let advAutoOpen = $derived(
    !loaded ||
      endpointError !== '' ||
      settings.token.trim() !== '' ||
      (settings.provider === 'openai' &&
        !(
          ['', OPENAI_DEFAULT_ENDPOINT].includes(settings.endpoint.trim()) &&
          ['', OPENAI_DEFAULT_MODEL].includes(settings.model.trim())
        )) ||
      (settings.provider === 'qwen' &&
        !(
          ['', QWEN_DEFAULT_ENDPOINT].includes(settings.endpoint.trim()) &&
          ['', QWEN_DEFAULT_MODEL].includes(settings.model.trim())
        )) ||
      (settings.provider === 'google' &&
        !(settings.endpoint.trim() === '' && settings.model.trim() === '')),
  );
  $effect(() => {
    if (advAutoOpen) advOpen = true;
  });

  $effect(() => {
    invoke<Settings>('get_settings')
      .then((s) => {
        settings = {
          ...s,
          theme: s.theme ?? 'system',
          inputDevice: s.inputDevice ?? '',
          typingSpeedWpm: s.typingSpeedWpm ?? 40,
          statsEnabled: s.statsEnabled ?? true,
          locale: s.locale ?? 'auto',
        };
        loaded = true;
      })
      .catch((e) => {
        loadingError = String(e);
        loaded = true;
      });
  });

  $effect(() => {
    if (!loaded) return;
    // регистрируем зависимость от всех полей
    void [
      settings.provider,
      settings.endpoint,
      settings.token,
      settings.model,
      settings.language,
      settings.hotkey,
      settings.insertDelayMs,
      settings.autostart,
      settings.theme,
      settings.inputDevice,
      settings.typingSpeedWpm,
      settings.statsEnabled,
      settings.locale,
    ];
    if (endpointError || hotkeyError) return;
    const snapshot: Settings = { ...settings };
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      invoke('save_settings', { settings: snapshot })
        .then(() => {
          saveError = '';
          saved = true;
          clearTimeout(flashTimer);
          flashTimer = setTimeout(() => (saved = false), 1000);
        })
        .catch((e) => (saveError = String(e)));
    }, 400);
    return () => clearTimeout(saveTimer);
  });

  $effect(() => {
    if (active !== 'stats') return;
    loadStats();
    const onFocus = () => loadStats();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  });

  // Применение темы: system — снимаем data-theme, дальше работает prefers-color-scheme
  $effect(() => {
    const t = settings.theme;
    if (t === 'system') delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = t;
  });

  const providerHint = $derived(
    settings.provider === 'qwen'
      ? t('ui.provider.hint_qwen')
      : settings.provider === 'google' && settings.language === ''
        ? t('ui.provider.hint_google_auto')
        : '',
  );
</script>

<div class="shell">
  <aside class="sidebar">
    <div class="brand">
      <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
        <rect x="2" y="10" width="2.6" height="4" rx="1.3" fill="url(#g)" />
        <rect x="7" y="6" width="2.6" height="12" rx="1.3" fill="url(#g)" />
        <rect x="12" y="2.5" width="2.6" height="19" rx="1.3" fill="url(#g)" />
        <rect x="17" y="7.5" width="2.6" height="9" rx="1.3" fill="url(#g)" />
        <rect x="21.4" y="10.5" width="2.2" height="3" rx="1.1" fill="url(#g)" />
        <defs>
          <linearGradient id="g" x1="0" y1="0" x2="24" y2="24">
            <stop offset="0" stop-color="#4f6ef7" />
            <stop offset="1" stop-color="#28c8e6" />
          </linearGradient>
        </defs>
      </svg>
      <span class="brand-name">VoiceDo</span>
      {#if appVersion}<span class="brand-ver">v{appVersion}</span>{/if}
    </div>

    <nav>
      {#each SECTIONS as [id, icon] (id)}
        <button class="nav-item" class:active={active === id} onclick={() => (active = id)}>
          <span class="nav-icon">{icon}</span>
          {t(`ui.section.${id}`)}
        </button>
      {/each}
    </nav>

    <div class="sidebar-foot">
      <span class="saved" class:visible={saved}>{t('ui.saved')}</span>
      <div class="theme-seg" role="radiogroup" aria-label={t('ui.theme.aria')}>
        {#each [['system', '◐'], ['light', '☀'], ['dark', '☾']] as [themeVal, glyph] (themeVal)}
          <button
            class="theme-btn"
            class:active={settings.theme === themeVal}
            role="radio"
            aria-checked={settings.theme === themeVal}
            title={t(`ui.theme.${themeVal}`)}
            onclick={() => (settings.theme = themeVal as Theme)}
          >
            {glyph}
          </button>
        {/each}
      </div>
      <label class="locale-field">
        <span class="locale-lbl">{t('ui.locale.label')}</span>
        <select class="locale-select" bind:value={settings.locale}>
          {#each LOCALES as [code, labelKey] (code)}
            <option value={code}>{labelKey ? t(labelKey) : t(`ui.lang.${code}`)}</option>
          {/each}
        </select>
      </label>
    </div>
  </aside>

  <main class="panel">
    <h1>{t(`ui.section.${active}`)}</h1>

    {#if loadingError}
      <p class="error">{t('ui.error.load_settings', { error: loadingError })}</p>
    {/if}
    {#if saveError}
      <p class="error">{t('ui.error.save_settings', { error: saveError })}</p>
    {/if}

    {#if active === 'connect'}
      <form class="stack" autocomplete="off" onsubmit={(e) => e.preventDefault()}>
        <div class="prov-grid">
          {#each ['openai', 'qwen', 'google'] as id (id)}
            <label class="prov-card" class:selected={settings.provider === id}>
              <input
                type="radio"
                name="provider"
                value={id}
                checked={settings.provider === id}
                onchange={() => onProviderChange(id as Provider)}
              />
              <span class="prov-title">{t(`ui.provider.${id}.title`)}</span>
              <span class="prov-desc">{t(`ui.provider.${id}.desc`)}</span>
              <span class="prov-badge">{t(`ui.provider.${id}.badge`)}</span>
            </label>
          {/each}
        </div>

        {#if settings.provider === 'google'}
          <p class="hint google-hint">
            {t('ui.google.hint')}{#if settings.language === ''}{t('ui.google.auto_hint')}{/if}
          </p>
        {/if}

        <div class="field">
          <label for="language">{t('ui.language.label')}</label>
          <select id="language" bind:value={settings.language} title={providerHint}>
            {#each LANGUAGE_CODES as code}
              <option value={code}>{code === '' ? t('ui.lang.auto') : t(`ui.lang.${code}`)}</option>
            {/each}
          </select>
        </div>

        <details class="adv" bind:open={advOpen}>
          <summary>{t('ui.advanced')}</summary>
          <div class="field">
            <label for="endpoint">{t('ui.endpoint.label')}</label>
            <input
              id="endpoint"
              type="text"
              bind:value={settings.endpoint}
              placeholder={settings.provider === 'google'
                ? t('ui.endpoint.ph_none')
                : settings.provider === 'qwen'
                  ? QWEN_DEFAULT_ENDPOINT
                  : 'https://api.openai.com/v1'}
            />
            {#if endpointError}
              <span class="error">{t(endpointError)}</span>
            {/if}
          </div>

          <div class="field">
            <label for="token">{t('ui.token.label')}</label>
            <div class="token-row">
              <input
                id="token"
                type={showToken ? 'text' : 'password'}
                bind:value={settings.token}
                placeholder={settings.provider === 'google' ? t('ui.token.ph_none') : 'sk-...'}
              />
              <button
                type="button"
                class="eye"
                aria-label={showToken ? t('ui.token.hide') : t('ui.token.show')}
                onclick={() => (showToken = !showToken)}
              >
                👁
              </button>
            </div>
          </div>

          <div class="field">
            <div class="row">
              <div class="field grow">
                <label for="model">{t('ui.model.label')}</label>
                <input
                  id="model"
                  type="text"
                  bind:value={settings.model}
                  placeholder={settings.provider === 'google'
                    ? t('ui.endpoint.ph_none')
                    : settings.provider === 'qwen'
                      ? QWEN_DEFAULT_MODEL
                      : 'whisper-1'}
                />
              </div>
              <button type="button" class="check" onclick={testConnection} disabled={checking}>
                {checking ? t('ui.check.testing') : t('ui.check.button')}
              </button>
            </div>
            {#if checkResult}
              {@const success = t('ui.check.success', { ms: formatInt(checkResult.latencyMs) })}
              <span
                class="check-line {checkResult.ok ? 'check-ok' : 'check-err'}"
                title={checkResult.ok
                  ? `${success}${checkResult.text ? `, «${checkResult.text}»` : ''}`
                  : checkResult.error ?? ''}
              >
                {#if checkResult.ok}
                  ✓ {success}{#if checkResult.text}, {checkResult.text}{/if}
                {:else}
                  ✗ {checkResult.error}
                {/if}
              </span>
            {/if}
          </div>
        </details>
      </form>
    {:else if active === 'input'}
      <form class="stack" autocomplete="off" onsubmit={(e) => e.preventDefault()}>
        <div class="field">
          <label for="mic">{t('ui.mic.label')}</label>
          <div class="row delay-row">
            <select
              id="mic"
              bind:value={settings.inputDevice}
              title={devices.map((d) => `${d.name} — ${d.formats.join(', ')}`).join('\n')}
            >
              <option value="">{t('ui.mic.system_default')}</option>
              {#each devices as d (d.name)}
                <option value={d.name}>{d.name}{d.isDefault ? t('ui.mic.default_suffix') : ''}</option>
              {/each}
            </select>
            <button class="btn" type="button" onclick={loadDevices} title={t('ui.mic.refresh')}>↻</button>
          </div>
          {#if devicesError}
            <span class="error">{t('ui.mic.list_error', { error: devicesError })}</span>
          {:else if settings.inputDevice && !devices.some((d) => d.name === settings.inputDevice)}
            <span class="hint">{t('ui.mic.device_missing')}</span>
          {/if}
        </div>

        <div class="field">
          <span class="lbl">{t('ui.hotkey.label')}</span>
          <HotkeyInput bind:value={settings.hotkey} />
          {#if hotkeyError}
            <span class="error">{t(hotkeyError)}</span>
          {:else}
            <span class="hint">{t('ui.hotkey.hint', { hotkey: OS_DEFAULT_HOTKEY })}</span>
          {/if}
        </div>

        <div class="field">
          <label for="delay-range">{t('ui.delay.label')}</label>
          <div class="row delay-row">
            <input
              id="delay-range"
              class="range"
              type="range"
              min="0"
              max="500"
              step="10"
              bind:value={settings.insertDelayMs}
            />
            <input
              id="delay"
              class="delay-num"
              type="number"
              min="0"
              max="500"
              step="10"
              bind:value={settings.insertDelayMs}
            />
            <span class="unit">{t('ui.delay.unit')}</span>
          </div>
        </div>
      </form>
    {:else if active === 'autostart'}
      <div class="stack">
        <label class="big-switch">
          <input type="checkbox" bind:checked={settings.autostart} />
          <span class="track"><span class="thumb"></span></span>
          <span class="switch-text">
            <b>{t('ui.autostart.title')}</b>
            <span class="hint">{t('ui.autostart.hint')}</span>
          </span>
        </label>
      </div>
    {:else if active === 'stats'}
      <div class="stack stats">
        <label class="big-switch">
          <input type="checkbox" bind:checked={settings.statsEnabled} />
          <span class="track"><span class="thumb"></span></span>
          <span class="switch-text">
            <b>{t('ui.stats.switch_title')}</b>
            <span class="hint">{t('ui.stats.switch_hint')}</span>
          </span>
        </label>

        {#if !settings.statsEnabled}
          <p class="hint">{t('ui.stats.off')}</p>
        {:else if statsError}
          <p class="error">{t('ui.stats.load_error', { error: statsError })}</p>
        {:else if stats}
          <div class="hero-card">
            <div class="hero-num">≈ {fmtMinutes(stats.minutesSavedToday)}</div>
            <div class="hero-label">{t('ui.stats.saved_caption')}</div>
            <div class="hero-sub">{t('ui.stats.saved_total', { time: fmtMinutes(stats.minutesSavedTotal) })}</div>
          </div>

          <div class="stat-grid">
            <div class="stat-card">
              <span class="stat-num">{formatInt(stats.today.words)}</span>
              <span class="stat-lbl">{t('ui.stats.words_today')}</span>
            </div>
            <div class="stat-card">
              <span class="stat-num">{formatInt(stats.weekWords)}</span>
              <span class="stat-lbl">{t('ui.stats.week')}</span>
            </div>
            <div class="stat-card">
              <span class="stat-num">{formatInt(stats.lifetime.words)}</span>
              <span class="stat-lbl">{t('ui.stats.lifetime')}</span>
            </div>
            <div class="stat-card">
              <span class="stat-num">{formatInt(stats.streakDays)} 🔥</span>
              <span class="stat-lbl">{t('ui.stats.streak')}</span>
            </div>
          </div>

          <div class="chart-card">
            <div class="chart-head">
              <span class="stat-lbl">{t('ui.stats.chart_title')}</span>
              <span class="stat-lbl">{t('ui.stats.record', { n: formatInt(stats.bestDayWords) })}</span>
            </div>
            <svg class="chart" viewBox="0 0 280 64" role="img" aria-label={t('ui.stats.chart_aria')}>
              {#each stats.chart as p, i (p.date)}
                {@const h = Math.max(2, (p.words / chartMax) * 56)}
                <rect
                  x={i * 20 + 3}
                  y={64 - h}
                  width="14"
                  height={h}
                  rx="2"
                  class:today-bar={i === stats.chart.length - 1}
                >
                  <title>{t('ui.stats.bar_title', { date: p.date, words: `${formatInt(p.words)} ${tPlural('tray.words', p.words)}` })}</title>
                </rect>
              {/each}
            </svg>
            <div class="chart-axis">
              <span>{stats.chart[0]?.date.slice(8)}</span>
              <span>{t('ui.stats.today')}</span>
            </div>
          </div>

          <div class="field">
            <label for="wpm">{t('ui.stats.wpm_label')}</label>
            <div class="row delay-row">
              <input
                id="wpm"
                class="range"
                type="range"
                min="20"
                max="80"
                step="5"
                bind:value={settings.typingSpeedWpm}
              />
              <input
                id="wpm-num"
                class="delay-num"
                type="number"
                min="20"
                max="80"
                step="5"
                bind:value={settings.typingSpeedWpm}
              />
              <span class="unit">{t('ui.stats.wpm_unit')}</span>
            </div>
            <span class="hint">{t('ui.stats.wpm_hint')}</span>
          </div>

          <p class="hint">
            {t('ui.stats.today_line', {
              sessions: formatInt(stats.today.sessions),
              minutes: formatDecimal(Math.round(stats.today.audioSec / 6) / 10),
            })}
          </p>
        {:else}
          <p class="hint">{t('ui.loading')}</p>
        {/if}
      </div>
    {:else}
      <MicTest />
    {/if}
  </main>
</div>

<style>
  .shell {
    display: flex;
    height: 100dvh;
    background: var(--bg);
    color: var(--fg);
  }

  .sidebar {
    flex: 0 0 180px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 10px;
    background: var(--panel);
    border-right: 1px solid var(--border);
    box-sizing: border-box;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 6px 6px;
  }

  .brand-name {
    font-size: 0.95rem;
    font-weight: 700;
    background: var(--grad);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }

  .brand-ver {
    font-size: 0.68rem;
    color: var(--muted, #8a8f98);
    align-self: center;
  }

  nav {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 8px;
    font: inherit;
    font-size: 0.82rem;
    text-align: left;
    padding: 8px 10px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .nav-item:hover {
    background: var(--accent-soft);
    color: var(--fg);
  }

  .nav-item.active {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .nav-icon {
    width: 18px;
    text-align: center;
  }

  .sidebar-foot {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0 4px;
  }

  .saved {
    font-size: 0.7rem;
    color: var(--ok);
    visibility: hidden;
    height: 1em;
  }

  .saved.visible {
    visibility: visible;
  }

  .theme-seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }

  .theme-btn {
    flex: 1;
    font: inherit;
    font-size: 0.85rem;
    padding: 5px 0;
    border: none;
    background: var(--input);
    color: var(--muted);
    cursor: pointer;
  }

  .theme-btn.active {
    background: var(--grad);
    color: #fff;
  }

  .locale-field {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .locale-lbl {
    font-size: 0.66rem;
    color: var(--muted);
  }

  .locale-select {
    font-size: 0.72rem;
    padding: 3px 6px;
  }

  .panel {
    flex: 1;
    min-width: 0;
    padding: 16px 20px;
    overflow-y: auto;
    box-sizing: border-box;
  }

  h1 {
    font-size: 1rem;
    font-weight: 650;
    margin: 0 0 12px;
  }

  .stack {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 520px;
  }

  .row {
    display: flex;
    gap: 8px;
    align-items: flex-end;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .grow {
    flex: 1;
  }

  label {
    font-size: 0.7rem;
    color: var(--muted);
  }

  .lbl {
    font-size: 0.7rem;
    color: var(--muted);
  }

  input,
  select {
    font: inherit;
    font-size: 0.82rem;
    padding: 6px 8px;
    border: 1px solid var(--border-strong);
    border-radius: 7px;
    background: var(--input);
    color: var(--fg);
    width: 100%;
    box-sizing: border-box;
  }

  input:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  .prov-grid {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  .prov-card {
    position: relative;
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas:
      'title badge'
      'desc desc';
    gap: 1px 8px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--card);
    cursor: pointer;
    box-shadow: var(--shadow);
  }

  .prov-card input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }

  .prov-card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent), var(--shadow);
    background: color-mix(in srgb, var(--accent) 5%, var(--card));
  }

  .prov-title {
    grid-area: title;
    font-size: 0.85rem;
    font-weight: 650;
  }

  .prov-desc {
    grid-area: desc;
    font-size: 0.7rem;
    color: var(--muted);
  }

  .prov-badge {
    grid-area: badge;
    font-size: 0.62rem;
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    white-space: nowrap;
  }

  .adv {
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--card);
    padding: 8px 12px;
  }

  .adv summary {
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--muted);
    cursor: pointer;
  }

  .adv[open] summary {
    margin-bottom: 8px;
  }

  .error {
    font-size: 0.68rem;
    color: var(--err);
    margin: 0;
  }

  .hint {
    font-size: 0.68rem;
    color: var(--muted);
    margin: 0;
  }

  .google-hint {
    color: var(--warn);
  }

  .token-row {
    display: flex;
    gap: 4px;
  }

  .token-row input {
    flex: 1;
  }

  .eye {
    font-size: 0.85rem;
    padding: 0 8px;
    border: 1px solid var(--border-strong);
    border-radius: 7px;
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }

  .check {
    flex-shrink: 0;
    font: inherit;
    font-size: 0.8rem;
    padding: 6px 12px;
    border: 1px solid var(--border-strong);
    border-radius: 7px;
    background: var(--input);
    color: var(--fg);
    cursor: pointer;
    white-space: nowrap;
  }

  .check:disabled {
    cursor: wait;
    opacity: 0.6;
  }

  .check-line {
    display: block;
    font-size: 0.72rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .check-ok {
    color: var(--ok);
  }

  .check-err {
    color: var(--err);
  }

  .delay-row {
    align-items: center;
    gap: 10px;
  }

  .range {
    flex: 1;
    accent-color: var(--accent);
    padding: 0;
    border: none;
    background: transparent;
  }

  .delay-num {
    width: 74px;
    flex: none;
  }

  .unit {
    font-size: 0.72rem;
    color: var(--muted);
  }

  .big-switch {
    display: flex;
    align-items: center;
    gap: 12px;
    cursor: pointer;
  }

  .big-switch input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }

  .track {
    flex: none;
    width: 46px;
    height: 26px;
    border-radius: 999px;
    background: var(--border-strong);
    position: relative;
    transition: background 0.15s;
  }

  .thumb {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
    transition: transform 0.15s;
  }

  .big-switch input:checked + .track {
    background: var(--grad);
  }

  .big-switch input:checked + .track .thumb {
    transform: translateX(20px);
  }

  .switch-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 0.82rem;
    color: var(--fg);
  }

  .hero-card {
    padding: 16px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--card);
    box-shadow: var(--shadow);
    text-align: center;
  }

  .hero-num {
    font-size: 1.9rem;
    font-weight: 750;
    background: var(--grad);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }

  .hero-label {
    font-size: 0.78rem;
    margin-top: 2px;
  }

  .hero-sub {
    font-size: 0.68rem;
    color: var(--muted);
    margin-top: 4px;
  }

  .stat-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 7px;
  }

  .stat-card {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--card);
    box-shadow: var(--shadow);
  }

  .stat-num {
    font-size: 1.05rem;
    font-weight: 700;
  }

  .stat-lbl {
    font-size: 0.66rem;
    color: var(--muted);
  }

  .chart-card {
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--card);
    box-shadow: var(--shadow);
  }

  .chart-head {
    display: flex;
    justify-content: space-between;
    margin-bottom: 6px;
  }

  .chart {
    width: 100%;
    height: 64px;
    display: block;
  }

  .chart rect {
    fill: var(--accent-soft);
  }

  .chart rect.today-bar {
    fill: var(--accent);
  }

  .chart-axis {
    display: flex;
    justify-content: space-between;
    font-size: 0.62rem;
    color: var(--muted);
    margin-top: 4px;
  }
</style>
