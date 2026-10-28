import { getLang, t } from './i18n';

/** Presentation only. Never change IPC error values or protocol comparisons. */
export function errorText(error: unknown): string {
  const raw = String(error);
  if (raw.endsWith('This legacy message has no original sender ID; deletion for both sides cannot be verified')) {
    return t('chat.legacy_delete_unverifiable');
  }
  // Stable native markers only. Keep platform diagnostics and IPC values out
  // of the translated message, and leave unknown technical errors intact.
  const marker = raw.startsWith('Error: ') ? raw.slice(7) : raw;
  if ((marker.startsWith('OS credential provider unavailable: ') &&
      marker.endsWith('; unlock or provision protected OS credentials before opening GIPNY')) ||
      marker === 'Android protected credential provider could not initialize' ||
      marker === 'Android credential initialization closed' ||
      marker === 'Android protected credential provider is not ready') {
    return t('auth.os_credentials_unavailable');
  }
  if (marker === 'device credential missing or inaccessible; raw profile copies cannot be opened; use an explicitly encrypted export/import') {
    return t('auth.device_credentials_missing');
  }
  if (marker === 'profile belongs to another OS credential provider; use an explicitly encrypted export/import') {
    return t('auth.profile_other_device');
  }
  if (marker === 'legacy unbound profile is unsupported; data was not deleted' ||
      marker === 'legacy plaintext headless database is unsupported; data was not deleted') {
    return t('auth.profile_legacy_unsupported');
  }
  if (marker === 'invalid device-binding metadata' ||
      marker === 'device credential does not authenticate this profile; data and password attempt counter were not changed') {
    return t('auth.profile_binding_invalid');
  }
  if (getLang() === 'ru') return raw;
  const detail: Record<string, string> = {
    'содержимое не соответствует расширению': 'contents do not match the extension',
    'нет начала изображения': 'missing image start',
    'ожидался маркер сегмента': 'expected a segment marker',
    'неожиданный маркер': 'unexpected marker',
    'обрезан заголовок сегмента': 'truncated segment header',
    'сегмент выходит за конец файла': 'segment exceeds the file size',
    'нет конца изображения': 'missing image end',
    'обрезан заголовок чанка': 'truncated chunk header',
    'чанк выходит за конец файла': 'chunk exceeds the file size',
    'первый чанк не IHDR': 'the first chunk is not IHDR',
    'размер RIFF больше файла': 'RIFF size exceeds the file size',
    'файл зашифрован': 'the file is encrypted',
  };
  const hint = 'disable “remove attachment metadata” in Settings or send another file';
  const unsupported = raw.match(/^приватность вложений: из формата (.+) метаданные удалить нельзя — конвертируйте в JPEG или PNG, либо /);
  if (unsupported) return `Attachment privacy: cannot remove metadata from ${unsupported[1]}. Convert to JPEG or PNG, or ${hint}`;
  const malformed = raw.match(/^приватность вложений: не удалось разобрать (.+) \((.+)\) — /);
  if (malformed) return `Attachment privacy: could not parse ${malformed[1]} (${detail[malformed[2]!] ?? malformed[2]}). ${hint}`;
  const large = raw.match(/^файл слишком большой: (.+) \((.+) МБ, лимит (.+) МБ\)$/);
  if (large) return `File too large: ${large[1]} (${large[2]} MB; limit ${large[3]} MB)`;
  const installer = raw.replace(/^unsupported: /, '');
  if (installer === 'установка отменена') return 'Installation cancelled';
  if (installer === 'нет графического запроса пароля (polkit) — поставьте пакет вручную')
    return 'No graphical password prompt (polkit). Install the package manually';
  return raw;
}

/** Only the backend's empty-log marker is UI text; never translate log lines. */
export function logText(raw: string): string {
  return !raw || raw === '(журнал пуст)' ? t('settings.log_is_empty') : raw;
}
