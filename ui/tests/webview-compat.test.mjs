// The Android build runs inside whatever WebView the phone has. China-market
// Honor/Huawei phones without Google services ship Huawei WebView 11 (Chromium
// 83) and never update past it. A bundle with newer syntax fails to parse there
// and the app opens as a blank page — rc3 did (2026-11-07). This holds the
// built Tauri UI (ui/dist) to that floor: every script parses without syntax
// Chromium 83 lacks, and no unshimmed newer API is referenced.
//
// Run after `npm run build`. The browser build (browser-dist) is not covered:
// it targets current browsers and uses top-level await by design.
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import * as acorn from 'acorn';
import * as walk from 'acorn-walk';

const dist = new URL('../dist/', import.meta.url).pathname;
const files = [join(dist, 'theme-init.js'), ...readdirSync(join(dist, 'assets')).filter(f => f.endsWith('.js')).map(f => join(dist, 'assets', f))];
if (files.length < 2) throw new Error('webview-compat: run `npm run build` first; dist is empty');

// APIs newer than Chromium 83 that nothing shims. Shimmed ones (src/compat.ts):
// replaceChildren, Object.hasOwn, crypto.randomUUID. Method names are checked
// only where they are called or constructed: the UI has data fields named `at`.
const deniedCalls = new Set([
  'at', 'findLast', 'findLastIndex', 'toSorted', 'toReversed', 'toSpliced', 'replaceAll',
  'any' /* Promise.any */, 'canParse', 'groupBy', 'fromAsync', 'withResolvers',
  'checkVisibility', 'union', 'intersection', 'difference', 'isWellFormed',
]);
const deniedConstructors = new Set(['Segmenter' /* Intl.Segmenter */, 'WeakRef', 'AggregateError', 'FinalizationRegistry']);
const deniedGlobals = new Set(['structuredClone', 'WeakRef', 'AggregateError', 'FinalizationRegistry', 'reportError', 'scheduler']);

const fnTypes = new Set(['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression']);
let failures = 0;
const fail = (file, what, node, src) => {
  failures++;
  const line = src.slice(0, node.start).split('\n').length;
  console.error(`webview-compat: ${file.replace(dist, 'dist/')}: ${what} at line ${line}: …${src.slice(Math.max(0, node.start - 40), node.start + 40)}…`);
};

for (const file of files) {
  const src = readFileSync(file, 'utf8');
  let ast;
  try {
    ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'module', allowHashBang: true });
  } catch (e) {
    try { ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'script', allowHashBang: true }); }
    catch (e2) { fail(file, `does not parse at all: ${e2.message}`, { start: e2.pos ?? 0 }, src); continue; }
  }
  walk.fullAncestor(ast, (node, _state, ancestors) => {
    switch (node.type) {
      case 'AssignmentExpression':
        if (['||=', '&&=', '??='].includes(node.operator)) fail(file, `logical assignment ${node.operator} (Chromium 85)`, node, src);
        break;
      case 'StaticBlock': fail(file, 'class static block (Chromium 94)', node, src); break;
      case 'MethodDefinition':
        if (node.key.type === 'PrivateIdentifier') fail(file, 'private method (Chromium 84)', node, src);
        break;
      case 'BinaryExpression':
        if (node.operator === 'in' && node.left.type === 'PrivateIdentifier') fail(file, '#x in obj (Chromium 91)', node, src);
        break;
      case 'AwaitExpression':
        if (!ancestors.slice(0, -1).some(a => fnTypes.has(a.type))) fail(file, 'top-level await (Chromium 89)', node, src);
        break;
      case 'ForOfStatement':
        if (node.await && !ancestors.slice(0, -1).some(a => fnTypes.has(a.type))) fail(file, 'top-level for await (Chromium 89)', node, src);
        break;
      case 'Literal':
        if (node.regex && /[dv]/.test(node.regex.flags)) fail(file, `regex flag ${node.regex.flags} (Chromium 90+)`, node, src);
        break;
      case 'ImportExpression':
        if (node.options || node.attributes) fail(file, 'import attributes (Chromium 123)', node, src);
        break;
      case 'CallExpression':
      case 'NewExpression': {
        const callee = node.callee;
        if (callee.type !== 'MemberExpression') break;
        const name = callee.computed ? (callee.property.type === 'Literal' ? callee.property.value : null) : callee.property.name;
        const denied = node.type === 'NewExpression' ? deniedConstructors : deniedCalls;
        if (typeof name === 'string' && denied.has(name)) fail(file, `.${name}() (newer than Chromium 83, no shim)`, node, src);
        break;
      }
      case 'Identifier':
        if (deniedGlobals.has(node.name)) {
          const parent = ancestors[ancestors.length - 2];
          const isProperty = parent && parent.type === 'MemberExpression' && parent.property === node && !parent.computed;
          const isKey = parent && (parent.type === 'Property' || parent.type === 'MethodDefinition' || parent.type === 'PropertyDefinition') && parent.key === node && !parent.computed;
          if (!isProperty && !isKey) fail(file, `global ${node.name} (newer than Chromium 83, no shim)`, node, src);
        }
        break;
    }
  });
}

if (failures) { console.error(`webview-compat: ${failures} finding(s); see WEBVIEW_FLOOR in vite.config.ts`); process.exit(1); }
console.log(`webview-compat: ${files.length} script(s) hold to Chromium 83`);
