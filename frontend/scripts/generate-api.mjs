import { readFile, writeFile } from 'node:fs/promises';
import openapiTS, { astToString } from 'openapi-typescript';
import { format } from 'prettier';

// Use the same formatting in generation and verification on every OS.
const output = new URL('../src/lib/api/generated.ts', import.meta.url);
const schema = new URL('../../docs/api/openapi.json', import.meta.url);
const types = astToString(await openapiTS(schema, { alphabetize: true }));
const source = await format(
  '// Generated from docs/api/openapi.json. Run npm run generate:api.\n' + types,
  { parser: 'typescript', singleQuote: true, trailingComma: 'all', printWidth: 100 },
);

if (process.argv.includes('--check')) {
  if ((await readFile(output, 'utf8')) !== source) {
    console.error('API types are stale. Run npm run generate:api and commit the result.');
    process.exitCode = 1;
  }
} else {
  await writeFile(output, source);
}
