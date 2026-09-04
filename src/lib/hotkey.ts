// Валидатор и утилиты хоткеев: общий для страницы и виджета записи.

// Модификаторы, которые понимает парсер global-hotkey на бэкенде
export const HK_MODIFIERS = new Set([
  'ALT',
  'OPTION',
  'CTRL',
  'CONTROL',
  'CMD',
  'COMMAND',
  'SUPER',
  'SHIFT',
  'COMMANDORCONTROL',
  'COMMANDORCTRL',
  'CMDORCTRL',
  'CMDORCONTROL',
]);

// Возвращает ключ каталога i18n ('' — валиден); текст — t(key) на странице.
export function validateHotkey(value: string): string {
  const v = value.trim();
  if (!v) return 'ui.validation.hotkey_empty';
  const tokens = v.split('+').map((t) => t.trim());
  if (tokens.some((t) => !t)) return 'ui.validation.hotkey_plus';
  const up = tokens.map((t) => t.toUpperCase());
  const mods = up.filter((t) => HK_MODIFIERS.has(t));
  const keys = up.filter((t) => !HK_MODIFIERS.has(t));
  if (mods.length === 0) return 'ui.validation.hotkey_mods';
  if (keys.length === 0) return 'ui.validation.hotkey_key';
  if (keys.length > 1) return 'ui.validation.hotkey_one_key';
  if (up[up.length - 1] !== keys[0]) return 'ui.validation.hotkey_order';
  return '';
}

// ОС-дефолт (совпадает с settings.rs на macOS)
export const OS_DEFAULT_HOTKEY = 'Cmd+Shift+Space';

const MOD_ORDER = ['Cmd', 'Ctrl', 'Alt', 'Shift'] as const;

const MOD_NAMES: Record<string, string> = {
  CMD: 'Cmd',
  COMMAND: 'Cmd',
  SUPER: 'Cmd',
  CTRL: 'Ctrl',
  CONTROL: 'Ctrl',
  ALT: 'Alt',
  OPTION: 'Alt',
  SHIFT: 'Shift',
};

// e.key → каноническое название для строки хоткея
export function keyName(key: string): string | null {
  switch (key) {
    case ' ':
      return 'Space';
    case 'Enter':
      return 'Enter';
    case 'Tab':
      return 'Tab';
    case 'Backspace':
      return 'Backspace';
    case 'Delete':
      return 'Delete';
    case 'Escape':
      return 'Esc';
    case 'ArrowUp':
      return 'Up';
    case 'ArrowDown':
      return 'Down';
    case 'ArrowLeft':
      return 'Left';
    case 'ArrowRight':
      return 'Right';
    case 'Home':
    case 'End':
    case 'PageUp':
    case 'PageDown':
      return key;
  }
  if (/^F\d{1,2}$/i.test(key)) return key.toUpperCase();
  if (key.length === 1) return key.toUpperCase();
  return null;
}

const KEY_LABELS: Record<string, string> = {
  Cmd: '⌘',
  Ctrl: '⌃',
  Alt: '⌥',
  Shift: '⇧',
  Space: '␣ Space',
  Esc: '⎋',
  Enter: '⏎',
  Up: '↑',
  Down: '↓',
  Left: '←',
  Right: '→',
};

// «Cmd+Shift+Space» → чипы для отображения; невалидные токены — как есть
export function hotkeyChips(value: string): string[] {
  const v = value.trim();
  if (!v) return [];
  return v
    .split('+')
    .map((t) => t.trim())
    .map((t) => {
      const up = t.toUpperCase();
      const mod = MOD_NAMES[up];
      if (mod) return KEY_LABELS[mod] ?? mod;
      const norm = keyName(t) ?? t;
      return KEY_LABELS[norm] ?? norm;
    });
}

// Из флагов клавиатуры + основной клавиши собрать строку «Cmd+Shift+Space»
export function buildHotkey(
  mods: { cmd: boolean; ctrl: boolean; alt: boolean; shift: boolean },
  key: string,
): string {
  const parts: string[] = [];
  if (mods.cmd) parts.push('Cmd');
  if (mods.ctrl) parts.push('Ctrl');
  if (mods.alt) parts.push('Alt');
  if (mods.shift) parts.push('Shift');
  if (key) parts.push(key);
  return parts.join('+');
}

export { MOD_ORDER };
