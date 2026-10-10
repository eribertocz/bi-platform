import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [tailwindcss(), sveltekit({ adapter: adapter({ fallback: '200.html' }) })],
  server: {
    proxy: {
      '/health': 'http://127.0.0.1:8080',
      '/api': 'http://127.0.0.1:8080',
    },
  },
  ...(process.env.VITEST ? { resolve: { conditions: ['browser'] } } : {}),
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    setupFiles: ['./src/test-setup.ts'],
    clearMocks: true,
    restoreMocks: true,
  },
});
