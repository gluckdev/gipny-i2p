// The WASM messenger (browser-dist) runs in whatever browser opens the page,
// and on iOS every browser is WebKit. Its profile vault needs OPFS synchronous
// access handles, which Safari gave synchronous methods only in 16.4 (Chromium
// 108, Firefox 111): BROWSER_FLOOR in vite.browser.config.ts. A bundle that
// parses only in a newer Safari, or calls an API 16.4 lacks, fails on a phone
// that could otherwise run it. This holds the built bundle to the floor: every
// script parses there, no newer API is used without a feature check, and
// top-level await stays in the entry module (Safari runs it partially until
// 27: a module several importers reach at once breaks, webkit.org/b/242740).
//
// A feature check is an optional call (`navigator.storage.estimate?.()`), an
// optional member (`navigator.connection?.type`) or a `typeof X` test. A plain
// call or bare reference of a newer name fails even inside `if ('x' in
// window)`, so the check is visible at the call site. Names are matched
// without their receiver, as in webview-compat: a UI method named like a
// newer built-in (`.union()`, `.groupBy()`) is flagged too — rename it or
// narrow the check to its receiver here rather than dropping the name.
//
// Versions are from MDN browser-compat-data 8.1.8 and the WebKit release
// notes. Run after `npm run build:browser`; skips when browser-dist is absent.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import * as acorn from 'acorn';
import * as walk from 'acorn-walk';

const ui = new URL('../', import.meta.url).pathname;
const dist = join(ui, 'browser-dist');

// One source of truth: the Safari entry of BROWSER_FLOOR.
const config = readFileSync(join(ui, 'vite.browser.config.ts'), 'utf8');
const floorList = /export const BROWSER_FLOOR = \[([^\]]*)\]/.exec(config)?.[1] ?? '';
const floor = /'safari(\d+(?:\.\d+)?)'/.exec(floorList)?.[1];
if (!floor) throw new Error('browser-compat: BROWSER_FLOOR in vite.browser.config.ts has no safari entry');
const ios = /'ios(\d+(?:\.\d+)?)'/.exec(floorList)?.[1];
if (ios !== floor) throw new Error(`browser-compat: BROWSER_FLOOR names safari${floor} but ios${ios}; every iOS browser is WebKit`);

const parts = version => version.split('.').map(Number);
const newer = (a, b) => { const [x, y] = [parts(a), parts(b)]; return x[0] !== y[0] ? x[0] > y[0] : (x[1] ?? 0) > (y[1] ?? 0); };
/** Safari lacks it at the floor: never shipped (null) or shipped later. */
const lacks = since => since === null || newer(since, floor);
const label = since => since === null ? 'no Safari has it' : `Safari ${since}`;

// Safari version that first had each name. Methods are checked only where
// called; `Receiver.name` keys pin static methods whose name an older instance
// method shares (`Promise.any` 14 vs `AbortSignal.any` 17.4).
const calls = {
  at: '15.4', findLast: '15.4', findLastIndex: '15.4', replaceAll: '13.1', randomUUID: '15.4', 'Object.hasOwn': '15.4',
  'Promise.any': '14', 'AbortSignal.timeout': '16', toSorted: '16', toReversed: '16', toSpliced: '16', with: '16',
  'Array.fromAsync': '16.4', fromAsync: '16.4', isWellFormed: '16.4', toWellFormed: '16.4', 'Atomics.waitAsync': '16.4', waitAsync: '16.4',
  union: '17', intersection: '17', difference: '17', symmetricDifference: '17', isSubsetOf: '17', isSupersetOf: '17', isDisjointFrom: '17',
  'URL.canParse': '17', canParse: '17', showPopover: '17', hidePopover: '17', togglePopover: '17', estimate: '17' /* StorageManager */, 'Response.json': '17',
  'Object.groupBy': '17.4', 'Map.groupBy': '17.4', groupBy: '17.4', 'Promise.withResolvers': '17.4', withResolvers: '17.4', 'AbortSignal.any': '17.4', checkVisibility: '17.4',
  'URL.parse': '18', 'Promise.try': '18.2', 'RegExp.escape': '18.2', toBase64: '18.2', toHex: '18.2', setFromBase64: '18.2', setFromHex: '18.2',
  'Uint8Array.fromBase64': '18.2', fromBase64: '18.2', 'Uint8Array.fromHex': '18.2', fromHex: '18.2',
  'Iterator.from': '18.4', 'Error.isError': '18.4', isError: '18.4', 'Math.sumPrecise': '26.2', sumPrecise: '26.2', createWritable: '26',
  requestIdleCallback: null, cancelIdleCallback: null, postTask: null, showSaveFilePicker: null, showOpenFilePicker: null, showDirectoryPicker: null,
};
const constructors = { Segmenter: '14.1', WeakRef: '14.1', FinalizationRegistry: '14.1', AggregateError: '14', DurationFormat: '16.4', CompressionStream: '16.4', DecompressionStream: '16.4', Float16Array: '18.2', DisposableStack: null, AsyncDisposableStack: null };
const globals = {
  structuredClone: '15.4', reportError: '15.4', BroadcastChannel: '15.4', WeakRef: '14.1', FinalizationRegistry: '14.1', AggregateError: '14',
  CompressionStream: '16.4', DecompressionStream: '16.4', Float16Array: '18.2', Iterator: '18.4',
  requestIdleCallback: null, cancelIdleCallback: null, scheduler: null, showSaveFilePicker: null, showOpenFilePicker: null, showDirectoryPicker: null,
  Temporal: null, DisposableStack: null, AsyncDisposableStack: null, SuppressedError: null,
};
const navigatorMembers = { connection: null, gpu: null };
// Iterator helpers share their names with Array methods; only a call chained
// straight onto an iterator source is one. Object.keys/values/entries return arrays.
const iteratorSources = new Set(['values', 'keys', 'entries', 'matchAll']);
const iteratorHelpers = new Set(['map', 'filter', 'take', 'drop', 'flatMap', 'reduce', 'toArray', 'forEach', 'some', 'every', 'find']);

const fnTypes = new Set(['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression']);
const propertyName = member => member.computed ? (member.property.type === 'Literal' ? member.property.value : null) : member.property.name;
/** Something the floor may lack, used so that its absence is handled. */
const guarded = (node, parent) => !!parent && (
  (parent.type === 'UnaryExpression' && parent.operator === 'typeof') ||
  (parent.type === 'CallExpression' && parent.callee === node && parent.optional) ||
  (parent.type === 'MemberExpression' && parent.object === node && parent.optional));

/** Findings for one built script: [{ what, line, excerpt }]. `entry`: the
 * module browser.html loads, the only place top-level await may live. */
function scan(src, entry) {
  const findings = [];
  const fail = (what, node) => findings.push({ what, line: src.slice(0, node.start).split('\n').length, excerpt: src.slice(Math.max(0, node.start - 40), node.start + 40).replace(/\s+/g, ' ') });
  let ast;
  try {
    ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'module', allowHashBang: true });
  } catch (e) {
    try { ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'script', allowHashBang: true }); }
    catch (e2) { fail(`does not parse at all: ${e2.message}`, { start: e2.pos ?? 0 }); return findings; }
  }
  walk.fullAncestor(ast, (node, _state, ancestors) => {
    const parent = ancestors[ancestors.length - 2];
    const topLevel = () => !ancestors.slice(0, -1).some(a => fnTypes.has(a.type));
    switch (node.type) {
      case 'AssignmentExpression':
        if (['||=', '&&=', '??='].includes(node.operator) && lacks('14')) fail(`logical assignment ${node.operator} (Safari 14)`, node);
        break;
      case 'StaticBlock': if (lacks('16.4')) fail('class static block (Safari 16.4)', node); break;
      case 'MethodDefinition':
        if (node.key.type === 'PrivateIdentifier' && lacks('15')) fail('private method (Safari 15)', node);
        break;
      case 'BinaryExpression':
        if (node.operator === 'in' && node.left.type === 'PrivateIdentifier' && lacks('15')) fail('#x in obj (Safari 15)', node);
        break;
      case 'AwaitExpression':
      case 'ForOfStatement':
        if (node.type === 'ForOfStatement' && !node.await) break;
        if (!topLevel()) break;
        if (!entry) fail('top-level await outside the entry module (Safari runs it partially until 27, webkit.org/b/242740)', node);
        else if (lacks('15')) fail('top-level await (Safari 15)', node);
        break;
      case 'Literal': {
        if (!node.regex) break;
        const { pattern, flags } = node.regex;
        if (flags.includes('v') && lacks('17')) fail('regex flag v (Safari 17)', node);
        if (flags.includes('d') && lacks('15')) fail('regex flag d (Safari 15)', node);
        if (/\(\?<[=!]/.test(pattern) && lacks('16.4')) fail('regex lookbehind (Safari 16.4)', node);
        if (/\(\?(?:[ims]+-?[ims]*|-[ims]+):/.test(pattern) && lacks('26')) fail('regex modifiers (Safari 26)', node);
        const names = [...pattern.matchAll(/\(\?<([^>=!][^>]*)>/g)].map(m => m[1]);
        if (new Set(names).size !== names.length && lacks('17')) fail('duplicate named groups (Safari 17)', node);
        break;
      }
      case 'ImportExpression':
        if ((node.options || node.attributes) && lacks('17.2')) fail('import attributes (Safari 17.2)', node);
        break;
      case 'ImportDeclaration':
      case 'ExportNamedDeclaration':
      case 'ExportAllDeclaration':
        if (node.attributes?.length && lacks('17.2')) fail('import attributes (Safari 17.2)', node);
        break;
      case 'CallExpression':
      case 'NewExpression': {
        const callee = node.callee;
        if (callee.type !== 'MemberExpression') break;
        const name = propertyName(callee);
        if (typeof name !== 'string') break;
        if (node.type === 'NewExpression') {
          if (Object.hasOwn(constructors, name) && lacks(constructors[name])) fail(`new .${name}() (${label(constructors[name])})`, node);
          break;
        }
        const receiver = callee.object.type === 'Identifier' ? `${callee.object.name}.${name}` : null;
        const key = receiver && Object.hasOwn(calls, receiver) ? receiver : Object.hasOwn(calls, name) ? name : null;
        if (key && lacks(calls[key]) && !node.optional) fail(`.${name}() (${label(calls[key])}, no feature check)`, node);
        const inner = callee.object;
        if (iteratorHelpers.has(name) && lacks('18.4') && inner.type === 'CallExpression' && inner.callee.type === 'MemberExpression' &&
            iteratorSources.has(propertyName(inner.callee)) && !(inner.callee.object.type === 'Identifier' && inner.callee.object.name === 'Object')) {
          fail(`iterator helper .${name}() (Safari 18.4)`, node);
        }
        break;
      }
      case 'MemberExpression': {
        if (node.object.type !== 'Identifier' || node.object.name !== 'navigator') break;
        const name = propertyName(node);
        if (Object.hasOwn(navigatorMembers, name) && lacks(navigatorMembers[name]) && !guarded(node, parent)) fail(`navigator.${name} (${label(navigatorMembers[name])}, no feature check)`, node);
        break;
      }
      case 'Identifier': {
        if (!Object.hasOwn(globals, node.name) || !lacks(globals[node.name])) break;
        const isProperty = parent && parent.type === 'MemberExpression' && parent.property === node && !parent.computed;
        const isKey = parent && (parent.type === 'Property' || parent.type === 'MethodDefinition' || parent.type === 'PropertyDefinition') && parent.key === node && !parent.computed;
        if (!isProperty && !isKey && !guarded(node, parent)) fail(`global ${node.name} (${label(globals[node.name])}, no feature check)`, node);
        break;
      }
    }
  });
  return findings;
}

const report = (file, findings) => findings.map(f => `browser-compat: ${file}: ${f.what} at line ${f.line}: …${f.excerpt}…`).join('\n');

test(`the scanner tells what Safari ${floor} lacks from what it has`, () => {
  const fixture = `
const a = [1].at(-1); const b = structuredClone({}); Object.hasOwn(a, 'x'); crypto.randomUUID(); Promise.any([]); AbortSignal.timeout(1);
[3, 1].toSorted(); 'x'.isWellFormed(); class K { static { K.n = 1; } } /(?<=a)b/.test('ab'); /(?<n>a)/d.test('a'); Object.entries(b).map(x => x);
const e = await navigator.storage.estimate?.(); if (typeof requestIdleCallback === 'function') {} navigator.connection?.type; globalThis.scheduler?.postTask?.(() => {});
Object.groupBy(a, x => x); Promise.withResolvers(); new Set().union(new Set()); URL.canParse('x');
AbortSignal.any([]); el.checkVisibility(); navigator.storage.estimate(); requestIdleCallback(() => {});
scheduler.postTask(() => {}); showSaveFilePicker(); navigator.connection.type; handle.createWritable();
new Uint8Array(1).toBase64(); Uint8Array.fromBase64('AA=='); m.values().map(x => x); new Float16Array(1); new Intl.Segmenter(); new globalThis.DisposableStack();
/[\\p{L}--[a-z]]/v.test(''); /(?i:a)/.test(''); /(?<n>a)|(?<n>b)/.test(''); import('./x.js', { with: { type: 'json' } });
`;
  const expected = [
    'top-level await outside the entry module (Safari runs it partially until 27, webkit.org/b/242740)',
    '.groupBy() (Safari 17.4, no feature check)', '.withResolvers() (Safari 17.4, no feature check)', '.union() (Safari 17, no feature check)', '.canParse() (Safari 17, no feature check)',
    '.any() (Safari 17.4, no feature check)', '.checkVisibility() (Safari 17.4, no feature check)', '.estimate() (Safari 17, no feature check)', 'global requestIdleCallback (no Safari has it, no feature check)',
    'global scheduler (no Safari has it, no feature check)', '.postTask() (no Safari has it, no feature check)', 'global showSaveFilePicker (no Safari has it, no feature check)',
    'navigator.connection (no Safari has it, no feature check)', '.createWritable() (Safari 26, no feature check)',
    '.toBase64() (Safari 18.2, no feature check)', '.fromBase64() (Safari 18.2, no feature check)', 'iterator helper .map() (Safari 18.4)', 'global Float16Array (Safari 18.2, no feature check)', 'new .DisposableStack() (no Safari has it)',
    'regex flag v (Safari 17)', 'regex modifiers (Safari 26)', 'duplicate named groups (Safari 17)', 'import attributes (Safari 17.2)',
  ];
  assert.deepEqual(scan(fixture, false).map(f => f.what).sort(), expected.sort());
  assert.deepEqual(scan(fixture, true).map(f => f.what).filter(what => what.startsWith('top-level')), [], 'the entry module may await at top level');
  assert.match(scan('using x = y;', false)[0].what, /does not parse at all/);
});

test(`the storage shape check names Safari/iOS ${floor}, the floor the build targets`, async () => {
  const { build } = await import('vite');
  const dir = await mkdtemp(join(tmpdir(), 'gipny-storage-'));
  try {
    await build({ configFile: false, logLevel: 'silent', root: ui, build: { outDir: dir, emptyOutDir: false, lib: { entry: 'src/browser-core/storage.ts', formats: ['es'], fileName: () => 'storage.mjs' } } });
    const storage = await import(pathToFileURL(join(dir, 'storage.mjs')).href);
    storage.assertSyncHandle({ getSize: () => 0 });
    storage.assertSyncHandle({ getSize: () => 16 * 1024 * 1024 });
    assert.throws(() => storage.assertSyncHandle({ getSize: () => Promise.resolve(0) }), new RegExp(`too old; the messenger needs Safari/iOS ${floor.replace('.', '\\.')} or newer`));
    assert.equal(storage.SYNC_HANDLE_FLOOR, `Safari/iOS ${floor}`);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test(`browser-dist holds to Safari ${floor}`, { skip: existsSync(join(dist, 'assets')) ? false : `no browser-dist/ here — nothing to hold to Safari ${floor}; \`npm run build:browser\` makes it` }, () => {
  const html = readFileSync(join(dist, 'browser.html'), 'utf8');
  const entry = /<script type="module"[^>]*src="[^"]*\/([^"/]+\.js)"/.exec(html)?.[1];
  assert.ok(entry, 'browser.html names its module entry');
  const files = [...readdirSync(dist).filter(f => f.endsWith('.js')), ...readdirSync(join(dist, 'assets')).filter(f => f.endsWith('.js')).map(f => join('assets', f))];
  assert.ok(files.length >= 4, 'browser-dist holds the entry, the worker, the shell worker and theme-init');
  const findings = files.flatMap(file => scan(readFileSync(join(dist, file), 'utf8'), file === join('assets', entry)).map(f => ({ file, ...f })));
  assert.equal(findings.length, 0, `${findings.length} finding(s); see BROWSER_FLOOR in vite.browser.config.ts\n${findings.map(f => report(f.file, [f])).join('\n')}`);
  console.log(`browser-compat: ${files.length} script(s) hold to Safari ${floor}; top-level await only in ${entry}`);
});
