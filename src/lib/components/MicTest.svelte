<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  // --- 6.6: панель проверки диктовки (микрофон → ASR, без хоткея и вставки).
  // Hold/toggle-схема и seq-фильтр перенесены из +page.svelte без изменений.

  type DictationResult = {
    seq: number;
    ok: boolean;
    text: string | null;
    error: string | null;
    latencyMs: number;
  };

  let micPhase = $state<'idle' | 'recording' | 'processing'>('idle');
  let micResult = $state('');
  let micError = $state('');
  let micElapsed = $state(0);
  let micSeq = $state(0);
  let pressStartedAt = 0;
  let recordStartedAt = 0;
  let micTick: ReturnType<typeof setInterval> | undefined;

  function micStartTick() {
    recordStartedAt = Date.now();
    micElapsed = 0;
    clearInterval(micTick);
    micTick = setInterval(() => (micElapsed = (Date.now() - recordStartedAt) / 1000), 100);
  }
  function micStopTick() {
    clearInterval(micTick);
    micTick = undefined;
  }

  async function micStart() {
    micError = '';
    try {
      micSeq = await invoke<number>('start_test_dictation');
      micPhase = 'recording';
      micStartTick();
    } catch (e) {
      micError = String(e);
      micPhase = 'idle';
    }
  }

  async function micStop() {
    micStopTick();
    micPhase = 'processing';
    try {
      await invoke('stop_test_dictation');
    } catch (e) {
      micError = String(e);
      micPhase = 'idle';
    }
  }

  // Схема кнопки: клик = переключатель (старт/стоп), удержание = push-to-talk.
  function onMicDown(e: PointerEvent) {
    const btn = e.currentTarget as HTMLElement | null;
    try {
      btn?.setPointerCapture(e.pointerId);
    } catch {
      /* указатель уже неактивен */
    }
    if (micPhase === 'processing') return;
    if (micPhase === 'recording') {
      pressStartedAt = 0;
      void micStop();
      return;
    }
    pressStartedAt = Date.now();
    void micStart();
  }

  function onMicUp() {
    if (micPhase === 'recording' && pressStartedAt && Date.now() - pressStartedAt >= 400) {
      pressStartedAt = 0;
      void micStop();
    }
    pressStartedAt = 0;
  }

  $effect(() => {
    const un = listen<DictationResult>('dictation-test-result', (event) => {
      const p = event.payload;
      if (p.seq < micSeq) return; // устаревший (тест перезапущен)
      micStopTick();
      micPhase = 'idle';
      if (p.ok) {
        micResult = p.text ?? '(пусто)';
        micError = '';
      } else {
        micResult = '';
        micError = p.error ?? 'Неизвестная ошибка';
      }
    });
    return () => {
      void un.then((f) => f());
      micStopTick();
    };
  });
</script>

<div class="mic-panel">
  <div class="mic-zone">
    <button
      class="mic-btn"
      class:recording={micPhase === 'recording'}
      class:processing={micPhase === 'processing'}
      disabled={micPhase === 'processing'}
      aria-pressed={micPhase === 'recording'}
      aria-label="Запись проверки диктовки"
      onpointerdown={onMicDown}
      onpointerup={onMicUp}
      onpointercancel={onMicUp}
      onlostpointercapture={onMicUp}
      oncontextmenu={(e) => e.preventDefault()}
    >
      {#if micPhase === 'recording'}
        <span class="glyph">■</span>
      {:else if micPhase === 'processing'}
        <span class="spinner"></span>
      {:else}
        <span class="glyph">🎙</span>
      {/if}
    </button>
    {#if micPhase === 'recording'}
      <span class="elapsed">{micElapsed.toFixed(1)} с</span>
    {:else if micPhase === 'processing'}
      <span class="elapsed">Распознаю…</span>
    {:else}
      <span class="elapsed">Держите и говорите</span>
    {/if}
  </div>

  {#if micError}
    <div class="bubble err" title={micError}>{micError}</div>
  {:else if micResult}
    <div class="bubble ok" title={micResult}>{micResult}</div>
  {/if}
  <p class="hint">Клик — вкл/выкл, удержание — пока держите. Вставка не выполняется.</p>
</div>

<style>
  .mic-panel {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding-top: 12px;
  }

  .mic-zone {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }

  .mic-btn {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    border: 1px solid var(--border-strong);
    background: var(--card);
    color: var(--fg);
    font-size: 1.5rem;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    touch-action: none;
    user-select: none;
    box-shadow: var(--shadow);
  }

  .mic-btn:disabled {
    cursor: wait;
    opacity: 0.8;
  }

  .mic-btn.recording {
    background: var(--err);
    border-color: var(--err);
    color: #fff;
    animation: pulse 1s ease-in-out infinite;
  }

  .mic-btn.processing {
    border-color: var(--accent);
    color: var(--accent);
  }

  .glyph {
    line-height: 1;
  }

  .spinner {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 3px solid var(--accent-soft);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      transform: scale(1);
      opacity: 1;
    }
    50% {
      transform: scale(1.06);
      opacity: 0.75;
    }
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .elapsed {
    font-size: 0.74rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .bubble {
    max-width: 90%;
    padding: 8px 12px;
    border-radius: 12px;
    font-size: 0.82rem;
    line-height: 1.35;
    word-break: break-word;
    white-space: pre-wrap;
    box-shadow: var(--shadow);
  }

  .bubble.ok {
    background: var(--accent-soft);
    border: 1px solid var(--border);
    color: var(--fg);
    border-bottom-left-radius: 4px;
  }

  .bubble.err {
    background: color-mix(in srgb, var(--err) 12%, var(--card));
    border: 1px solid color-mix(in srgb, var(--err) 40%, var(--border));
    color: var(--err);
  }

  .hint {
    font-size: 0.68rem;
    color: var(--muted);
    margin: 0;
  }
</style>
