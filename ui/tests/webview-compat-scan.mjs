import * as acorn from 'acorn';
import * as walk from 'acorn-walk';

// APIs newer than Chromium 83 that nothing shims. Shimmed ones (src/compat.ts):
// replaceChildren, Object.hasOwn, crypto.randomUUID. Method names are checked
// only where they are called or constructed: the UI has data fields named `at`.
// The names are matched without their receiver, so a UI method of the same name
// (`.any()`, `.at()`, `.union()`) would be flagged too; rename it or narrow the
// check to the receiver here rather than dropping the name.
const deniedCalls = new Set([
  'at', 'findLast', 'findLastIndex', 'toSorted', 'toReversed', 'toSpliced', 'replaceAll',
  'any' /* Promise.any */, 'canParse', 'groupBy', 'fromAsync', 'withResolvers',
  'checkVisibility', 'union', 'intersection', 'difference', 'isWellFormed',
]);
const deniedConstructors = new Set(['Segmenter' /* Intl.Segmenter */, 'WeakRef', 'AggregateError', 'FinalizationRegistry']);
const deniedGlobals = new Set(['structuredClone', 'WeakRef', 'AggregateError', 'FinalizationRegistry', 'reportError', 'scheduler']);
const fnTypes = new Set(['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression']);

/** Findings for one script that must remain parseable by Chromium 83. */
export function scanWebviewCompatibility(src) {
  const findings = [];
  const fail = (what, node) => findings.push({
    what,
    line: src.slice(0, node.start).split('\n').length,
    excerpt: src.slice(Math.max(0, node.start - 40), node.start + 40).replace(/\s+/g, ' '),
  });
  let ast;
  try {
    ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'module', allowHashBang: true });
  } catch (e) {
    try { ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'script', allowHashBang: true }); }
    catch (e2) {
      fail(`does not parse at all: ${e2.message}`, { start: e2.pos ?? 0 });
      return findings;
    }
  }

  walk.fullAncestor(ast, (node, _state, ancestors) => {
    switch (node.type) {
      case 'VariableDeclaration':
        if (node.kind === 'using' || node.kind === 'await using') {
          fail(`${node.kind} declaration (explicit resource management, newer than Chromium 83)`, node);
        }
        break;
      case 'AssignmentExpression':
        if (['||=', '&&=', '??='].includes(node.operator)) fail(`logical assignment ${node.operator} (Chromium 85)`, node);
        break;
      case 'StaticBlock':
        fail('class static block (Chromium 94)', node);
        break;
      case 'MethodDefinition':
        if (node.key.type === 'PrivateIdentifier') fail('private method (Chromium 84)', node);
        break;
      case 'BinaryExpression':
        if (node.operator === 'in' && node.left.type === 'PrivateIdentifier') fail('#x in obj (Chromium 91)', node);
        break;
      case 'AwaitExpression':
        if (!ancestors.slice(0, -1).some(a => fnTypes.has(a.type))) fail('top-level await (Chromium 89)', node);
        break;
      case 'ForOfStatement':
        if (node.await && !ancestors.slice(0, -1).some(a => fnTypes.has(a.type))) fail('top-level for await (Chromium 89)', node);
        break;
      case 'Literal':
        if (node.regex && /[dv]/.test(node.regex.flags)) fail(`regex flag ${node.regex.flags} (Chromium 90+)`, node);
        break;
      case 'ImportExpression':
        if (node.options || node.attributes) fail('import attributes (Chromium 123)', node);
        break;
      case 'ImportDeclaration':
      case 'ExportNamedDeclaration':
      case 'ExportAllDeclaration':
        if (node.attributes?.length || node.assertions?.length) fail('import attributes (Chromium 123)', node);
        break;
      case 'CallExpression':
      case 'NewExpression': {
        const callee = node.callee;
        if (callee.type !== 'MemberExpression') break;
        const name = callee.computed ? (callee.property.type === 'Literal' ? callee.property.value : null) : callee.property.name;
        const denied = node.type === 'NewExpression' ? deniedConstructors : deniedCalls;
        if (typeof name === 'string' && denied.has(name)) fail(`.${name}() (newer than Chromium 83, no shim)`, node);
        break;
      }
      case 'Identifier': {
        if (!deniedGlobals.has(node.name)) break;
        const parent = ancestors[ancestors.length - 2];
        const isProperty = parent && parent.type === 'MemberExpression' && parent.property === node && !parent.computed;
        const isKey = parent && (parent.type === 'Property' || parent.type === 'MethodDefinition' || parent.type === 'PropertyDefinition') && parent.key === node && !parent.computed;
        if (!isProperty && !isKey) fail(`global ${node.name} (newer than Chromium 83, no shim)`, node);
        break;
      }
    }
  });

  return findings;
}
