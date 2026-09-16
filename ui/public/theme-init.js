// Applies the saved theme before first paint, so a dark-theme user does not see
// a flash of light. Must be a separate file rather than inline: the app's CSP is
// script-src 'self', which forbids inline scripts. Mirrors ui/src/theme.ts.
(function () {
  var t = 'light';
  try {
    var v = localStorage.getItem('gipny.theme');
    if (v === 'light' || v === 'dark' || v === 'system') t = v;
  } catch (e) { /* storage unavailable: keep the default */ }
  document.documentElement.setAttribute('data-theme', t);
})();
