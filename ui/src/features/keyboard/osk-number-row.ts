import type { LayoutId } from './osk-layouts';

const DIGITS = [...'1234567890'];
// Latin digits are intentional on every layout. Shift follows the selected
// letter layout: Khmer NiDA and Hindi InScript levels follow xkeyboard-config
// kh(basic)/in(deva); Vietnamese Telex and Chinese pinyin use US QWERTY.
const SHIFT: Readonly<Record<LayoutId, readonly string[]>> = {
  en: [...'!@#$%^&*()'],
  ru: [...'!"№;%:?*()'],
  uk: [...'!"№;%:?*()'],
  de: [...'!"§$%&/()='],
  vi: [...'!@#$%^&*()'],
  zh: [...'!@#$%^&*()'],
  hi: [...'ऍॅ#$%^&*()'],
  km: [...'!ៗ"៛%៍័៏()'],
};

/** Caps lock affects letters; only one-shot Shift changes this digit row. */
export function numberRow(layout: LayoutId, shift: boolean): readonly string[] {
  return shift ? SHIFT[layout] : DIGITS;
}
