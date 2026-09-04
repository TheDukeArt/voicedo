<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import '../lib/theme.css';
  import { i18n, type ResolvedLocale } from '$lib/i18n/index.svelte';
  import type { Snippet } from 'svelte';

  let { children }: { children: Snippet } = $props();

  // Разрешённая локаль — источник истины Rust; смена мгновенная через событие.
  $effect(() => {
    invoke<ResolvedLocale>('get_locale')
      .then((loc) => (i18n.locale = loc))
      .catch(() => {});
    const un = listen<ResolvedLocale>('locale-changed', (e) => (i18n.locale = e.payload));
    return () => {
      void un.then((f) => f());
    };
  });

  $effect(() => {
    document.documentElement.lang = i18n.locale;
  });
</script>

{@render children()}
