import { readFileSync, readdirSync } from 'node:fs';
import { gzipSync } from 'node:zlib';

// Run after `npm run build`. The route graph includes shared runtime chunks
// (possibly cached); excludes CSS, source maps and shell entrypoints.
const output = '.svelte-kit/output/client/';
const route = process.argv[2] ?? 'explorar';
/** @type {Record<string, { file: string; name?: string; isDynamicEntry?: boolean; imports?: string[]; dynamicImports?: string[] }>} */
const manifest = JSON.parse(readFileSync(`${output}.vite/manifest.json`, 'utf8'));
const nodes = '.svelte-kit/generated/build/client-optimized/nodes/';
const routeNode = readdirSync(nodes).find((name) =>
  readFileSync(nodes + name, 'utf8').includes(`src/routes/${route}/+page.svelte`),
);
if (!routeNode) throw new Error(`Build the ${route} route before measuring.`);
const root = Object.keys(manifest).find(
  (key) => manifest[key]?.name === `nodes/${routeNode.replace('.js', '')}`,
);
if (!root) throw new Error(`${route} route missing in the client manifest.`);
/** @param {string} key @param {boolean} dynamic @param {Set<string>} seen */
function dependencies(key, dynamic = false, seen = new Set()) {
  if (seen.has(key)) return seen;
  seen.add(key);
  const entry = manifest[key];
  if (!entry) throw new Error(`Missing chunk: ${key}`);
  for (const child of [
    ...(entry.imports ?? []),
    ...(dynamic ? (entry.dynamicImports ?? []) : []),
  ]) {
    dependencies(child, dynamic, seen);
  }
  return seen;
}
/** @param {Set<string>} keys */
function measure(keys) {
  const files = [...keys].map((key) => {
    const file = manifest[key]?.file;
    if (!file) throw new Error(`Missing file: ${key}`);
    const bytes = readFileSync(output + file);
    return { file, raw: bytes.length, gzip: gzipSync(bytes).length };
  });
  return {
    raw: files.reduce((n, f) => n + f.raw, 0),
    gzip: files.reduce((n, f) => n + f.gzip, 0),
    files,
  };
}
const chart = 'src/lib/features/semantic/bars.ts';
if (!manifest[chart]?.isDynamicEntry) throw new Error('ECharts must have its own dynamic entry.');
const initial = dependencies(root);
if (initial.has(chart)) throw new Error('ECharts must stay behind a dynamic import.');
if (route !== 'explorar') {
  if (dependencies(root, true).has(chart)) throw new Error('ECharts reached an unrelated route.');
  console.log(JSON.stringify({ route, initial: measure(initial) }, null, 2));
  process.exit(0);
}
for (const name of readdirSync(nodes).filter((name) => name !== routeNode)) {
  const key = Object.keys(manifest).find(
    (key) => manifest[key]?.name === `nodes/${name.replace('.js', '')}`,
  );
  if (key && dependencies(key, true).has(chart))
    throw new Error(`ECharts loaded outside exploration: ${name}`);
}
console.log(
  JSON.stringify(
    { initial: measure(initial), withChart: measure(dependencies(root, true)) },
    null,
    2,
  ),
);
