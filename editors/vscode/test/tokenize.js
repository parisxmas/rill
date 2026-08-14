#!/usr/bin/env node
// Tokenizes Rill sources with the extension's TextMate grammar.
//
//   npm install && npm test          # assert the expectations below
//   node test/tokenize.js a.rill     # dump every token and its scope
//
// The dump is the quickest way to see what the grammar does to a file without
// opening an editor; inside VS Code the same information comes from
// `Developer: Inspect Editor Tokens and Scopes`.

const fs = require('fs');
const path = require('path');
const vsctm = require('vscode-textmate');
const oniguruma = require('vscode-oniguruma');

const ROOT = path.join(__dirname, '..');
const GRAMMAR = path.join(ROOT, 'syntaxes', 'rill.tmLanguage.json');
const FIXTURE = path.join(__dirname, 'fixture.rill');

// [source text, token, the scope it must carry (a prefix is enough)]
const EXPECTATIONS = [
  ['extern "m" fn sqrt(x: Float) -> Float', 'extern', 'keyword.other.extern'],
  ['extern "m" fn sqrt(x: Float) -> Float', 'sqrt', 'entity.name.function'],
  ['extern "m" fn sqrt(x: Float) -> Float', 'Float', 'support.type.primitive'],
  ['type Shape', 'type', 'storage.type'],
  ['type Shape', 'Shape', 'entity.name.type'],
  ['  Circle(r: Float)', 'Circle', 'entity.name.type.constructor'],
  ['trait Describe(a)', 'Describe', 'entity.name.type.trait'],
  ['impl Describe(Shape)', 'impl', 'storage.type.impl'],
  ['impl Show(Box(a))', 'Show', 'entity.name.type.trait'],
  ['fn show(x) = "x"', 'show', 'entity.name.function'],
  ['fn describe(s) = "shape " + show(s)', 'fn', 'storage.type.function'],
  ['fn describe(s) = "shape " + show(s)', 'describe', 'entity.name.function'],
  ['fn describe(s) = "shape " + show(s)', 'show', 'support.function.builtin'],
  ['  a = "hash # inside a string"', 'hash # inside a string', 'string.quoted.double'],
  ['  b = "escapes \\n \\r \\t \\\\ \\" end"', '\\n', 'constant.character.escape'],
  ['fn ops(a, b) = a + b - a * b / 2 % 3', '%', 'keyword.operator.arithmetic'],
  ['fn ops(a, b) = a + b - a * b / 2 % 3', '2', 'constant.numeric.integer'],
  ['fn logic(a, b) = a >= b && !(a != b) || a <= b', '>=', 'keyword.operator.comparison'],
  ['fn logic(a, b) = a >= b && !(a != b) || a <= b', '&&', 'keyword.operator.logical'],
  ['fn annotated(v: Float, k: Float) -> Float = v * k', 'v', 'variable.parameter'],
  ['fn evens(n) = range(1, n) |> filter(\\x -> x) |> sum', '|>', 'keyword.operator.pipeline'],
  ['fn evens(n) = range(1, n) |> filter(\\x -> x) |> sum', 'filter', 'support.function.prelude'],
  ['fn evens(n) = range(1, n) |> filter(\\x -> x) |> sum', 'sum', 'support.function.prelude'],
  ['fn evens(n) = range(1, n) |> filter(\\x -> x) |> sum', '\\', 'storage.type.function.lambda'],
  ['  Cons(Some(0), _) -> "starts with zero"', 'Cons', 'support.constant.prelude'],
  ['  Cons(Some(0), _) -> "starts with zero"', '_', 'variable.language.wildcard'],
  ['  Cons(Some(0), _) -> "starts with zero"', '->', 'keyword.operator.arrow'],
  ['fn poll(ch) = select', 'select', 'keyword.control.concurrency'],
  ['  default -> -1', 'default', 'keyword.control.default'],
  ['  spawn produce(ch, 9)', 'spawn', 'keyword.control.concurrency'],
  ['  println(classify(l))', 'println', 'support.function.builtin'],
  ['  println(classify(l))', 'classify', 'entity.name.function.call'],
  ['import "metric" as m', 'import', 'keyword.control.import'],
  ['import "metric" as m', 'as', 'keyword.control.import'],
  ['import "metric" as m', 'm', 'entity.name.namespace'],
  ['fn f() = m.label(m.Meters(1.5))', 'm', 'entity.name.namespace'],
  ['fn f() = m.label(m.Meters(1.5))', 'label', 'entity.name.function.call'],
  ['fn f() = m.label(m.Meters(1.5))', 'Meters', 'entity.name.type.constructor'],
  ['fn f() = m.label(m.Meters(1.5))', '1.5', 'constant.numeric.float'],
  [
    '# a comment with "a quote" and fn main() = 0',
    ' a comment with "a quote" and fn main() = 0',
    'comment.line',
  ],
  ['fn f() = "bad escape \\q here"', '\\q', 'invalid.illegal.unknown-escape'],
  ['fn f() = true', 'true', 'constant.language.boolean'],
  ['fn f(x) = if x then 1 else 2', 'if', 'keyword.control.conditional'],
];

async function loadGrammar() {
  const wasm = fs.readFileSync(
    path.join(ROOT, 'node_modules/vscode-oniguruma/release/onig.wasm')
  );
  await oniguruma.loadWASM(wasm.buffer);
  const registry = new vsctm.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (s) => new oniguruma.OnigScanner(s),
      createOnigString: (s) => new oniguruma.OnigString(s),
    }),
    loadGrammar: async (scope) =>
      scope === 'source.rill'
        ? vsctm.parseRawGrammar(fs.readFileSync(GRAMMAR, 'utf8'), GRAMMAR)
        : null,
  });
  const grammar = await registry.loadGrammar('source.rill');
  if (!grammar) throw new Error('could not load ' + GRAMMAR);
  return grammar;
}

/** Every (text, scope) pair on one line, ignoring whitespace-only tokens. */
function tokensOf(grammar, line, ruleStack) {
  const r = grammar.tokenizeLine(line, ruleStack);
  const out = [];
  for (const t of r.tokens) {
    const text = line.substring(t.startIndex, t.endIndex);
    if (text.trim()) out.push({ text, scopes: t.scopes });
  }
  return { tokens: out, ruleStack: r.ruleStack };
}

function dump(grammar, files) {
  for (const file of files) {
    console.log('==== ' + file);
    let rules = vsctm.INITIAL;
    for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
      const r = tokensOf(grammar, line, rules);
      rules = r.ruleStack;
      if (!r.tokens.length) continue;
      const parts = r.tokens.map(
        (t) => JSON.stringify(t.text) + ' → ' + t.scopes[t.scopes.length - 1]
      );
      console.log('  ' + parts.join('\n  '));
    }
  }
}

function check(grammar) {
  let failed = 0;
  for (const [line, needle, expected] of EXPECTATIONS) {
    const { tokens } = tokensOf(grammar, line, vsctm.INITIAL);
    const hit = tokens.find((t) => t.text === needle);
    if (!hit) {
      console.error(`FAIL  ${JSON.stringify(needle)} is not a token of: ${line}`);
      failed++;
      continue;
    }
    const scope = hit.scopes[hit.scopes.length - 1];
    if (!scope.startsWith(expected)) {
      console.error(
        `FAIL  ${JSON.stringify(needle)} in: ${line}\n      want ${expected}*, got ${scope}`
      );
      failed++;
    }
  }

  // The fixture is the whole language in one file: nothing in it may fall
  // through unscoped, and nothing in it is an error.
  let rules = vsctm.INITIAL;
  let lineNo = 0;
  for (const line of fs.readFileSync(FIXTURE, 'utf8').split('\n')) {
    lineNo++;
    const r = tokensOf(grammar, line, rules);
    rules = r.ruleStack;
    for (const t of r.tokens) {
      const scope = t.scopes[t.scopes.length - 1];
      if (scope === 'source.rill') {
        console.error(`FAIL  fixture.rill:${lineNo}: ${JSON.stringify(t.text)} has no scope`);
        failed++;
      }
      if (scope.startsWith('invalid.')) {
        console.error(`FAIL  fixture.rill:${lineNo}: ${JSON.stringify(t.text)} is ${scope}`);
        failed++;
      }
    }
  }

  if (failed) {
    console.error(`\n${failed} failing expectation(s)`);
    process.exit(1);
  }
  console.log(`ok — ${EXPECTATIONS.length} expectations, fixture.rill fully scoped`);
}

loadGrammar()
  .then((grammar) => {
    const files = process.argv.slice(2);
    if (files.length) dump(grammar, files);
    else check(grammar);
  })
  .catch((e) => {
    console.error(e);
    process.exit(1);
  });
