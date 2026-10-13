import { render, screen, fireEvent } from '@testing-library/svelte';
import { beforeEach, expect, it, vi } from 'vitest';
import Page from './+page.svelte';
import { ApiError } from '#lib/api/session.ts';

const mocks = vi.hoisted(() => ({ login: vi.fn(), startCorporateLogin: vi.fn(), goto: vi.fn() }));
vi.mock('$app/navigation', () => ({ goto: mocks.goto }));
vi.mock('#lib/api/session.ts', async (original) => ({
  ...(await original<typeof import('#lib/api/session.ts')>()),
  getDevUsers: vi.fn().mockResolvedValue([{ id: 'analyst', display_name: 'Analista' }]),
  getOidcConfig: vi.fn().mockResolvedValue(true),
  login: mocks.login,
  startCorporateLogin: mocks.startCorporateLogin,
}));
beforeEach(() => vi.clearAllMocks());

it.each([
  [
    new ApiError(403, 'r', 'origin_forbidden', 'http://127.0.0.1:8080'),
    'Esta dirección no está autorizada',
  ],
  [new ApiError(401, 'r', 'unknown_dev_user'), 'El usuario no existe'],
  [new ApiError(429, 'r', 'rate_limited'), 'Demasiados intentos'],
  [new ApiError(503), 'El servidor no está disponible'],
  [new TypeError('Failed to fetch'), 'El servidor no responde'],
  [new ApiError(400), 'No se pudo iniciar sesión'],
])('explains login rejection %s', async (cause, message) => {
  mocks.login.mockRejectedValue(cause);
  render(Page);
  await screen.findByLabelText('Usuario de desarrollo');
  await fireEvent.click(screen.getByRole('button', { name: /^Entrar$/ }));
  expect((await screen.findByRole('alert')).textContent).toContain(message);
  if (cause instanceof ApiError && cause.code === 'origin_forbidden') {
    expect(screen.getByRole('link', { name: cause.publicOrigin }).getAttribute('href')).toBe(
      cause.publicOrigin,
    );
    expect(screen.queryByText(/Intenta de nuevo/)).toBeNull();
    expect(screen.queryByRole('button', { name: 'Reintentar' })).toBeNull();
  } else {
    expect(screen.queryByRole('link')).toBeNull();
  }
});

it('also explains a forbidden corporate origin', async () => {
  mocks.startCorporateLogin.mockRejectedValue(
    new ApiError(403, 'r', 'origin_forbidden', 'https://bi.example.test'),
  );
  render(Page);
  await fireEvent.click(await screen.findByRole('button', { name: 'Entrar con mi empresa' }));
  expect((await screen.findByRole('alert')).textContent).toContain(
    'Esta dirección no está autorizada',
  );
  expect(screen.getByRole('link').getAttribute('href')).toBe('https://bi.example.test');
});
