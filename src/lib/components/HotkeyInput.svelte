<script lang="ts">
  import { buildHotkey, hotkeyChips, keyName, OS_DEFAULT_HOTKEY } from '$lib/hotkey';
  import { t } from '$lib/i18n/index.svelte';

  let { value = $bindable() }: { value?: string } = $props();

  type Phase = 'display' | 'recording';
  let phase = $state<Phase>('display');
  let pending = $state(''); // черновик «Cmd+Shift+» во время записи

  function startRecording() {
    phase = 'recording';
    pending = '';
  }

  function cancel() {
    phase = 'display';
    pending = '';
  }

  function commit(candidate: string) {
    value = candidate;
    phase = 'display';
    pending = '';
  }

  function onKeyDown(e: KeyboardEvent) {
    if (phase !== 'recording') return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === 'Escape') {
      cancel(); // отмена без изменений
      return;
    }
    if (e.key === 'Backspace') {
      commit(OS_DEFAULT_HOTKEY); // сброс к ОС-дефолту (валидатор на странице проверит)
      return;
    }
    if (e.key === 'Enter') {
      if (pending) commit(pending);
      else cancel();
      return;
    }

    const mods = { cmd: e.metaKey, ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey };
    const isModifierOnly = ['Meta', 'Control', 'Alt', 'Shift', 'OS'].includes(e.key);
    const name = isModifierOnly ? null : keyName(e.key);

    // ждём модификаторов: обычная клавиша без модификаторов не принимается
    if (!mods.cmd && !mods.ctrl && !mods.alt && !mods.shift) {
      pending = '';
      return;
    }
    if (!name) {
      // пока зажаты только модификаторы — показываем черновик и ждём клавишу
      pending = buildHotkey(mods, '');
      return;
    }
    commit(buildHotkey(mods, name));
  }

  $effect(() => {
    if (phase !== 'recording') return;
    const el = box;
    el?.focus();
  });

  let box = $state<HTMLDivElement | null>(null);
</script>

<div
  bind:this={box}
  class="hk {phase}"
  role="button"
  tabindex="0"
  aria-label={t('ui.hotkey.aria')}
  onclick={phase === 'display' ? startRecording : undefined}
  onkeydown={onKeyDown}
  onblur={() => phase === 'recording' && cancel()}
>
  {#if phase === 'recording'}
    {#if pending}
      <span class="chips">
        {#each hotkeyChips(pending + '…') as chip (chip)}
          <span class="chip">{chip}</span>
        {/each}
      </span>
    {:else}
      <span class="prompt">{t('ui.hotkey.prompt')}</span>
    {/if}
  {:else if value}
    <span class="chips">
      {#each hotkeyChips(value) as chip, i (i)}
        <span class="chip">{chip}</span>
      {/each}
    </span>
  {:else}
    <span class="prompt">{t('ui.hotkey.set_prompt')}</span>
  {/if}
</div>

<style>
  .hk {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 34px;
    padding: 4px 8px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--input);
    cursor: pointer;
    user-select: none;
    box-sizing: border-box;
    width: 100%;
  }

  .hk:focus-visible,
  .hk.recording {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
    cursor: default;
  }

  .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .chip {
    font-size: 0.74rem;
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 5px;
    background: var(--chip);
    border: 1px solid var(--border);
    color: var(--fg);
    white-space: nowrap;
  }

  .prompt {
    font-size: 0.74rem;
    color: var(--muted);
  }
</style>
