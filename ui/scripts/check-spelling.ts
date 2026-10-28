// UI copy is US English (optimization, not optimisation). Scans what a user can read (string literals, template text,
// regex literals, since tests match copy with them, and JSX text) in common/src, e2e/ and every edition's src/ for
// British spellings. Identifiers and comments are not copy and are skipped, as are module specifiers (they name files).
// ponytail: a word list, not a dictionary; switch to cspell with en-US if a spelling outside SPELLING slips through.
// Usage: node scripts/check-spelling.ts [file…]   (default: every .ts/.tsx under the roots below)
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'
import { findEditions } from './editions.ts'

const UI = join(dirname(fileURLToPath(import.meta.url)), '..')
const ROOTS = ['common/src', 'e2e', ...findEditions(UI).map((e) => `${e.dir}/src`)]
const SKIP_DIR = /^(node_modules|dist|\.tanstack)$/

/** British stem → the US spelling to use instead. */
const SPELLING: [RegExp, string][] = [
  [/optimis/i, 'optimiz'],
  [/prioritis/i, 'prioritiz'],
  [/analys(e|ed|es|ing)\b/i, 'analyz…'],
  [/colour/i, 'color'],
  [/behaviour/i, 'behavior'],
]

/**
 * Words that are names, not copy, kept under a path segment. The genui DSL's component and tool names are emitted by
 * the model and stored in past conversations' programs, so renaming them would break those renders.
 */
const ALLOW: [string, string[]][] = [['/genui/', ['Optimisation', 'fetchOptimisation']]]

const WORD = /[\p{L}\p{N}_]+/gu

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) {
      if (!SKIP_DIR.test(name)) yield* files(path)
    } else if (/\.tsx?$/.test(name) && !name.endsWith('.gen.ts')) yield path
  }
}

/** Every piece of readable text in a source file, with its line. */
function* copyIn(path: string): Generator<{ text: string; line: number }> {
  const src = ts.createSourceFile(path, readFileSync(path, 'utf8'), ts.ScriptTarget.Latest, true)
  const isSpecifier = ({ parent }: ts.Node) =>
    ts.isImportDeclaration(parent) ||
    ts.isExportDeclaration(parent) ||
    ts.isExternalModuleReference(parent) ||
    (ts.isCallExpression(parent) && parent.expression.kind === ts.SyntaxKind.ImportKeyword)
  const visit = function* (node: ts.Node): Generator<{ text: string; line: number }> {
    if (
      (ts.isStringLiteral(node) && !isSpecifier(node)) ||
      ts.isNoSubstitutionTemplateLiteral(node) ||
      ts.isTemplateHead(node) ||
      ts.isTemplateMiddle(node) ||
      ts.isTemplateTail(node) ||
      ts.isRegularExpressionLiteral(node) ||
      ts.isJsxText(node)
    )
      yield { text: node.text, line: src.getLineAndCharacterOfPosition(node.getStart()).line + 1 }
    for (const child of node.getChildren(src)) yield* visit(child)
  }
  yield* visit(src)
}

function findBritish(paths: string[]): string[] {
  const found: string[] = []
  for (const path of paths) {
    const rel = relative(UI, path)
    const allowed = new Set(
      ALLOW.flatMap(([segment, words]) => (rel.includes(segment) ? words : [])),
    )
    for (const { text, line } of copyIn(path))
      for (const [word] of text.matchAll(WORD)) {
        if (allowed.has(word)) continue
        const us = SPELLING.find(([re]) => re.test(word))?.[1]
        if (us) found.push(`${rel}:${line}: "${word}" → ${us}`)
      }
  }
  return found
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2)
  const paths = args.length ? args : ROOTS.flatMap((r) => [...files(join(UI, r))])
  const found = findBritish(paths)
  for (const f of found) console.error(f)
  if (found.length) {
    console.error(`\n${found.length} British spelling(s) in UI copy; UI copy is US English.`)
    process.exit(1)
  }
  console.log(`UI copy spelling: ${paths.length} files, US English.`)
}
