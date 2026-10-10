import js from '@eslint/js';
import globals from 'globals';
import svelte from 'eslint-plugin-svelte';
import ts from 'typescript-eslint';
import prettier from 'eslint-config-prettier';

export default ts.config(
  { ignores: ['node_modules/**', '.svelte-kit/**', 'build/**', 'src/lib/api/generated.ts'] },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs['flat/recommended'],
  { languageOptions: { globals: { ...globals.browser, ...globals.node } } },
  { files: ['**/*.svelte'], languageOptions: { parserOptions: { parser: ts.parser } } },
  prettier,
);
