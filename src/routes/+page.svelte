<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';

  type Provider = 'openai' | 'qwen';

  type Settings = {
    provider: Provider;
    endpoint: string;
    token: string;
    model: string;
    language: string;
    hotkey: string;
    insertDelayMs: number;
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

  let settings = $state<Settings>({
    provider: 'openai',
    endpoint: '',
    token: '',
    model: 'whisper-1',
    language: '',
    hotkey: 'Cmd+Shift+Space',
    insertDelayMs: 50,
  });
  let loaded = $state(false);
  let loadingError = $state('');
  let saveError = $state('');
  let showToken = $state(false);
  let saved = $state(false);
  let checking = $state(false);
  let checkResult = $state<{ ok: boolean; text: string | null; latencyMs: number; error: string | null } | null>(null);

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
    if (next === 'qwen') {
      if (ep === '' || ep === OPENAI_DEFAULT_ENDPOINT) {
        settings.endpoint = QWEN_DEFAULT_ENDPOINT;
      }
      if (settings.model.trim() === '' || settings.model === OPENAI_DEFAULT_MODEL) {
        settings.model = QWEN_DEFAULT_MODEL;
      }
    } else {
      if (ep === QWEN_DEFAULT_ENDPOINT) {
        settings.endpoint = '';
      }
      if (settings.model === QWEN_DEFAULT_MODEL) {
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

  // Модификаторы, которые понимает парсер global-hotkey на бэкенде
  const HK_MODIFIERS = new Set([
    'ALT', 'OPTION', 'CTRL', 'CONTROL', 'CMD', 'COMMAND', 'SUPER', 'SHIFT',
    'COMMANDORCONTROL', 'COMMANDORCTRL', 'CMDORCTRL', 'CMDORCONTROL',
  ]);

  function validateHotkey(value: string): string {
    const v = value.trim();
    if (!v) return 'Укажите сочетание, например Cmd+Shift+Space';
    const tokens = v.split('+').map((t) => t.trim());
    if (tokens.some((t) => !t)) return 'Лишний «+» или пустая часть — формат: Cmd+Shift+Space';
    const up = tokens.map((t) => t.toUpperCase());
    const mods = up.filter((t) => HK_MODIFIERS.has(t));
    const keys = up.filter((t) => !HK_MODIFIERS.has(t));
    if (mods.length === 0) return 'Добавьте модификаторы, например Cmd+Shift+Space';
    if (keys.length === 0) return 'Добавьте обычную клавишу, например Space или F12';
    if (keys.length > 1) return 'Только одна обычная клавиша, модификаторы — перед ней';
    if (up[up.length - 1] !== keys[0]) return 'Модификаторы должны идти перед клавишей: Cmd+Shift+Space';
    return '';
  }

  $effect(() => {
    invoke<Settings>('get_settings')
      .then((s) => {
        settings = s;
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
</script>

<main class="container">
  <h1>Настройки VoiceDo</h1>

  {#if loadingError}
    <p class="error">Не удалось загрузить настройки: {loadingError}</p>
  {/if}
  {#if saveError}
    <p class="error">Не удалось сохранить настройки: {saveError}</p>
  {/if}

  <form autocomplete="off" onsubmit={(e) => e.preventDefault()}>
    <div class="field">
      <label for="provider">Провайдер</label>
      <select
        id="provider"
        value={settings.provider}
        onchange={(e) => onProviderChange((e.target as HTMLSelectElement).value as Provider)}
      >
        <option value="openai">OpenAI-совместимый</option>
        <option value="qwen">Qwen (DashScope)</option>
      </select>
    </div>

    <div class="field">
      <label for="endpoint">Эндпоинт</label>
      <input
        id="endpoint"
        type="text"
        bind:value={settings.endpoint}
        placeholder={settings.provider === 'qwen'
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
          placeholder="sk-..."
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
      <button type="button" class="check" onclick={testConnection} disabled={checking}>
        {checking ? 'Проверка…' : 'Проверить подключение'}
      </button>
      {#if checkResult}
        {#if checkResult.ok}
          <span class="check-ok">
            ✓ Успех: {checkResult.latencyMs} мс,
            {#if checkResult.text?.startsWith('(')}
              {checkResult.text}
            {:else}
              распознано: {checkResult.text ? `«${checkResult.text}»` : '(пусто)'}
            {/if}
          </span>
        {:else}
          <span class="check-err">✗ {checkResult.error}</span>
        {/if}
      {/if}
    </div>

    <div class="row">
      <div class="field grow">
        <label for="model">Модель</label>
        <input
          id="model"
          type="text"
          bind:value={settings.model}
          placeholder={settings.provider === 'qwen' ? QWEN_DEFAULT_MODEL : 'whisper-1'}
        />
      </div>
      <div class="field grow">
        <label for="language">Язык</label>
        <select id="language" bind:value={settings.language}>
          {#each LANGUAGES as [code, name]}
            <option value={code}>{name}</option>
          {/each}
        </select>
        {#if settings.provider === 'qwen'}
          <span class="hint">
            Qwen (DashScope): язык пока не передаётся — выбор игнорируется для этого провайдера
          </span>
        {/if}
      </div>
    </div>

    <div class="row">
      <div class="field grow">
        <label for="hotkey">Хоткей</label>
        <input id="hotkey" type="text" bind:value={settings.hotkey} placeholder="Cmd+Shift+Space" />
        {#if hotkeyError}
          <span class="error">{hotkeyError}</span>
        {:else}
          <span class="hint">Модификаторы через «+», последняя клавиша — обычная, напр. Cmd+Shift+Space</span>
        {/if}
      </div>
      <div class="field delay">
        <label for="delay">Задержка вставки, мс</label>
        <input id="delay" type="number" min="0" step="10" bind:value={settings.insertDelayMs} />
      </div>
    </div>
  </form>

  <div class="status" aria-live="polite">
    {#if saved}Сохранено{/if}
  </div>
</main>

<style>
  .container {
    padding: 16px 20px;
  }

  h1 {
    font-size: 1.1rem;
    font-weight: 600;
    margin: 0 0 12px;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .row {
    display: flex;
    gap: 10px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-bottom: 0;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .delay {
    width: 130px;
  }

  label {
    font-size: 0.75rem;
    color: var(--color-muted, #666);
  }

  input,
  select {
    font: inherit;
    font-size: 0.85rem;
    padding: 6px 8px;
    border: 1px solid #bbb;
    border-radius: 6px;
    background: var(--color-bg-input, #fff);
    width: 100%;
    box-sizing: border-box;
  }

  input:focus,
  select:focus {
    outline: 2px solid #4a90d9;
    outline-offset: -1px;
  }

  .token-row {
    display: flex;
    gap: 4px;
  }

  .token-row input {
    flex: 1;
  }

  .eye {
    font-size: 0.9rem;
    padding: 0 8px;
    border: 1px solid #bbb;
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
  }

  .error {
    font-size: 0.72rem;
    color: #c0392b;
  }

  .hint {
    font-size: 0.72rem;
    color: var(--color-muted, #666);
  }

  .check {
    align-self: flex-start;
    font: inherit;
    font-size: 0.85rem;
    padding: 6px 12px;
    border: 1px solid #bbb;
    border-radius: 6px;
    background: var(--color-bg-input, #fff);
    cursor: pointer;
  }

  .check:disabled {
    cursor: wait;
    opacity: 0.6;
  }

  .check-ok {
    font-size: 0.78rem;
    color: #2e7d32;
  }

  .check-err {
    font-size: 0.78rem;
    color: #c0392b;
  }

  .status {
    height: 1.2em;
    margin-top: 8px;
    font-size: 0.8rem;
    color: #2e7d32;
  }
</style>
