import assert from 'node:assert/strict';
import { test } from 'node:test';
import { scanWebviewCompatibility } from './webview-compat-scan.mjs';

const findingsFor = src => scanWebviewCompatibility(src).map(({ what }) => what);
const mustDeny = (src, expected) => {
  const findings = findingsFor(src);
  assert.ok(!findings.some(what => what.startsWith('does not parse at all:')), `latest Acorn must parse: ${src}`);
  assert.ok(findings.includes(expected), `expected ${expected}; got ${findings.join(', ') || 'no findings'}`);
};

test('the Chromium 83 gate denies syntax newly accepted by latest Acorn', () => {
  mustDeny('using resource = acquire();', 'using declaration (explicit resource management, newer than Chromium 83)');
  mustDeny('await using resource = acquireAsync();', 'await using declaration (explicit resource management, newer than Chromium 83)');
});

test('the Chromium 83 gate retains denials for other newer parser syntax', () => {
  const fixture = `
    target ||= fallback;
    class Example { static { this.ready = true; } #method() {} }
    await start();
    import value from './data.json' with { type: 'json' };
    /value/d.test(value);
    /value/v.test(value);
  `;

  assert.deepEqual(findingsFor(fixture).sort(), [
    'class static block (Chromium 94)',
    'import attributes (Chromium 123)',
    'logical assignment ||= (Chromium 85)',
    'private method (Chromium 84)',
    'regex flag d (Chromium 90+)',
    'regex flag v (Chromium 90+)',
    'top-level await (Chromium 89)',
  ].sort());
});

test('the Chromium 83 gate permits valid floor syntax', () => {
  const fixture = `
    const value = maybe?.nested ?? fallback;
    async function outer() {
      return async function inner() {
        await work(value);
      };
    }
  `;

  assert.deepEqual(findingsFor(fixture), []);
});
