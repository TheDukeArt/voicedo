<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import HotkeyInput from '$lib/components/HotkeyInput.svelte';
  import MicTest from '$lib/components/MicTest.svelte';
  import { validateHotkey } from '$lib/hotkey';

  type Provider = 'openai' | 'qwen' | 'google';
  type Theme = 'system' | 'light' | 'dark';
  type Section = 'connect' | 'input' | 'autostart' | 'check';

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
  };

  const QWEN_DEFAULT_ENDPOINT =
    'https://dashscope-intl.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation';
  const OPENAI_DEFAULT_ENDPOINT = 'https://api.openai.com/v1';
  const QWEN_DEFAULT_MODEL = 'qwen-audio-3.0-asr-flash';
  const OPENAI_DEFAULT_MODEL = 'whisper-1';

  const LANGUAGES: [string, string][] = [
    ['', 'Автоопределение'],
    ['ru', 'Русский'],
    ['en', 'English'],
    ['es', 'Español'],
    ['fr', 'Français'],
    ['de', 'Deutsch'],
    ['it', 'Italiano'],
    ['pt', 'Português'],
    ['pl', 'Polski'],
    ['tr', 'Türkçe'],
    ['uk', 'Українська'],
    ['zh', '中文'],
    ['ja', '日本語'],
  ];

  const SECTIONS: [Section, string, string][] = [
    ['connect', '🔌', 'Подключение'],
    ['input', '⌨️', 'Ввод'],
    ['autostart', '🚀', 'Автозапуск'],
    ['check', '🎙', 'Проверка'],
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
  });
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

  function validateEndpoint(value: string): string {
    const v = value.trim();
    if (!v) return '';
    let url: URL;
    try {
      url = new URL(v);
    } catch {
      return 'Некорректный URL — укажите полный адрес, например https://api.openai.com/v1';
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      return 'Адрес должен начинаться с http:// или https://';
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
        settings = { ...s, theme: s.theme ?? 'system' };
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

  // Применение темы: system — снимаем data-theme, дальше работает prefers-color-scheme
  $effect(() => {
    const t = settings.theme;
    if (t === 'system') delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = t;
  });

  const providerHint = $derived(
    settings.provider === 'qwen'
      ? 'Qwen (DashScope): язык пока не передаётся — выбор игнорируется'
      : settings.provider === 'google' && settings.language === ''
        ? 'Google: при «автоопределении» реально уйдёт en-US'
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
    </div>

    <nav>
      {#each SECTIONS as [id, icon, name] (id)}
        <button class="nav-item" class:active={active === id} onclick={() => (active = id)}>
          <span class="nav-icon">{icon}</span>
          {name}
        </button>
      {/each}
    </nav>

    <div class="sidebar-foot">
      <span class="saved" class:visible={saved}>Сохранено</span>
      <div class="theme-seg" role="radiogroup" aria-label="Тема">
        {#each [['system', '◐'], ['light', '☀'], ['dark', '☾']] as [t, glyph] (t)}
          <button
            class="theme-btn"
            class:active={settings.theme === t}
            role="radio"
            aria-checked={settings.theme === t}
            title={t === 'system' ? 'Системная тема' : t === 'light' ? 'Светлая тема' : 'Тёмная тема'}
            onclick={() => (settings.theme = t as Theme)}
          >
            {glyph}
          </button>
        {/each}
      </div>
    </div>
  </aside>

  <main class="panel">
    <h1>{SECTIONS.find(([id]) => id === active)?.[2]}</h1>

    {#if loadingError}
      <p class="error">Не удалось загрузить настройки: {loadingError}</p>
    {/if}
    {#if saveError}
      <p class="error">Не удалось сохранить настройки: {saveError}</p>
    {/if}

    {#if active === 'connect'}
      <form class="stack" autocomplete="off" onsubmit={(e) => e.preventDefault()}>
        <div class="prov-grid">
          {#each [
            ['openai', 'OpenAI-совместимый', 'Whisper и любые /v1-совместимые серверы', 'API-ключ'],
            ['qwen', 'Qwen (DashScope)', 'Мультимодальная ASR Alibaba Cloud', 'API-ключ'],
            ['google', 'Google', 'Скрытый API Chrome · без ключа · ≤15 с', 'бесплатно, офф-рекорд'],
          ] as [id, title, desc, badge] (id)}
            <label class="prov-card" class:selected={settings.provider === id}>
              <input
                type="radio"
                name="provider"
                value={id}
                checked={settings.provider === id}
                onchange={() => onProviderChange(id as Provider)}
              />
              <span class="prov-title">{title}</span>
              <span class="prov-desc">{desc}</span>
              <span class="prov-badge">{badge}</span>
            </label>
          {/each}
        </div>

        {#if settings.provider === 'google'}
          <p class="hint google-hint">
            Google: до ~15 с за запись, без ключа; может перестать работать в любой момент.
            {#if settings.language === ''}Язык «авто» — реально уйдёт en-US.{/if}
          </p>
        {/if}

        <div class="field">
          <label for="language">Язык</label>
          <select id="language" bind:value={settings.language} title={providerHint}>
            {#each LANGUAGES as [code, name]}
              <option value={code}>{name}</option>
            {/each}
          </select>
        </div>

        <details class="adv" bind:open={advOpen}>
          <summary>Дополнительно</summary>
          <div class="field">
            <label for="endpoint">Эндпоинт</label>
            <input
              id="endpoint"
              type="text"
              bind:value={settings.endpoint}
              placeholder={settings.provider === 'google'
                ? 'не требуется'
                : settings.provider === 'qwen'
                  ? QWEN_DEFAULT_ENDPOINT
                  : 'https://api.openai.com/v1'}
            />
            {#if endpointError}
              <span class="error">{endpointError}</span>
            {/if}
          </div>

          <div class="field">
            <label for="token">Токен</label>
            <div class="token-row">
              <input
                id="token"
                type={showToken ? 'text' : 'password'}
                bind:value={settings.token}
                placeholder={settings.provider === 'google' ? 'не требуется (без ключа)' : 'sk-...'}
              />
              <button
                type="button"
                class="eye"
                aria-label={showToken ? 'Скрыть токен' : 'Показать токен'}
                onclick={() => (showToken = !showToken)}
              >
                👁
              </button>
            </div>
          </div>

          <div class="field">
            <div class="row">
              <div class="field grow">
                <label for="model">Модель</label>
                <input
                  id="model"
                  type="text"
                  bind:value={settings.model}
                  placeholder={settings.provider === 'google'
                    ? 'не требуется'
                    : settings.provider === 'qwen'
                      ? QWEN_DEFAULT_MODEL
                      : 'whisper-1'}
                />
              </div>
              <button type="button" class="check" onclick={testConnection} disabled={checking}>
                {checking ? 'Проверка…' : 'Проверить подключение'}
              </button>
            </div>
            {#if checkResult}
              <span
                class="check-line {checkResult.ok ? 'check-ok' : 'check-err'}"
                title={checkResult.ok
                  ? `Успех: ${checkResult.latencyMs} мс${checkResult.text ? `, «${checkResult.text}»` : ''}`
                  : checkResult.error ?? ''}
              >
                {#if checkResult.ok}
                  ✓ {checkResult.latencyMs} мс{#if checkResult.text}, {checkResult.text}{/if}
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
          <span class="lbl">Хоткей (удерживайте для записи)</span>
          <HotkeyInput bind:value={settings.hotkey} />
          {#if hotkeyError}
            <span class="error">{hotkeyError}</span>
          {:else}
            <span class="hint">Клик — задать, Backspace — сброс к {''}Cmd+Shift+Space, Esc — отмена.</span>
          {/if}
        </div>

        <div class="field">
          <label for="delay-range">Задержка перед вставкой</label>
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
            <span class="unit">мс</span>
          </div>
        </div>
      </form>
    {:else if active === 'autostart'}
      <div class="stack">
        <label class="big-switch">
          <input type="checkbox" bind:checked={settings.autostart} />
          <span class="track"><span class="thumb"></span></span>
          <span class="switch-text">
            <b>Запускать при входе в систему</b>
            <span class="hint">VoiceDo будет доступен по хоткею сразу после загрузки macOS.</span>
          </span>
        </label>
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
</style>
