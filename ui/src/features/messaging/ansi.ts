const COLORS = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'];

/** Render SGR styling as DOM nodes; command output is never parsed as HTML. */
export function renderAnsi(text: string): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const attributes = new Set<string>();
  let foreground = '', background = '';
  let foregroundRgb = '', backgroundRgb = '';
  const append = (value: string): void => {
    if (!value) return;
    const span = document.createElement('span');
    span.className = [...attributes, foreground, background].filter(Boolean).join(' ');
    if (foregroundRgb) span.style.color = foregroundRgb;
    if (backgroundRgb) span.style.backgroundColor = backgroundRgb;
    span.textContent = value;
    fragment.appendChild(span);
  };
  let offset = 0;
  for (const match of text.matchAll(/\x1b\[([0-9;]*)m/g)) {
    append(text.slice(offset, match.index));
    offset = match.index! + match[0].length;
    const codes = match[1]!.split(';').map((code) => Number(code));
    for (let i = 0; i < codes.length; i++) {
      const code = codes[i]!;
      if (code === 0) {
        attributes.clear();
        foreground = background = foregroundRgb = backgroundRgb = '';
      } else if (code === 1) attributes.add('ansi-bold');
      else if (code === 2) attributes.add('ansi-dim');
      else if (code === 3) attributes.add('ansi-italic');
      else if (code === 4) attributes.add('ansi-underline');
      else if (code === 9) attributes.add('ansi-strike');
      else if (code === 22) { attributes.delete('ansi-bold'); attributes.delete('ansi-dim'); }
      else if (code === 23) attributes.delete('ansi-italic');
      else if (code === 24) attributes.delete('ansi-underline');
      else if (code === 29) attributes.delete('ansi-strike');
      else if (code === 39) { foreground = foregroundRgb = ''; }
      else if (code === 49) { background = backgroundRgb = ''; }
      else if ((code >= 30 && code <= 37) || (code >= 90 && code <= 97)) {
        foreground = `ansi-${code >= 90 ? 'bright-' : ''}${COLORS[code % 10]}`;
        foregroundRgb = '';
      } else if ((code >= 40 && code <= 47) || (code >= 100 && code <= 107)) {
        background = `ansi-bg-${code >= 100 ? 'bright-' : ''}${COLORS[code % 10]}`;
        backgroundRgb = '';
      } else if (code === 38 || code === 48) {
        // Consume extended-color parameters together so they aren't read as SGR flags.
        const mode = codes[++i];
        const count = mode === 2 ? 3 : mode === 5 ? 1 : 0;
        const values = codes.slice(i + 1, i + 1 + count);
        i += count;
        if (!count || values.length !== count || values.some((v) => v < 0 || v > 255)) continue;
        let rgb = '';
        let colorClass = '';
        if (mode === 2) rgb = `rgb(${values.join(',')})`;
        else {
          const n = values[0]!;
          if (n < 16) colorClass = `${n >= 8 ? 'bright-' : ''}${COLORS[n % 8]}`;
          else if (n < 232) {
            const levels = [0, 95, 135, 175, 215, 255];
            const v = n - 16;
            rgb = `rgb(${levels[Math.floor(v / 36)]},${levels[Math.floor(v / 6) % 6]},${levels[v % 6]})`;
          } else rgb = `rgb(${Array(3).fill(8 + (n - 232) * 10).join(',')})`;
        }
        if (code === 38) { foreground = colorClass ? `ansi-${colorClass}` : ''; foregroundRgb = rgb; }
        else { background = colorClass ? `ansi-bg-${colorClass}` : ''; backgroundRgb = rgb; }
      }
    }
  }
  append(text.slice(offset));
  return fragment;
}
