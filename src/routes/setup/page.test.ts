import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import Page from './+page.svelte';

// vi.hoisted ensures mockGoto is available when the vi.mock factory runs (both are hoisted).
const mockGoto = vi.hoisted(() => vi.fn());

vi.mock('$app/navigation', () => ({ goto: mockGoto }));
vi.mock('@tauri-apps/api/path', () => ({ homeDir: vi.fn().mockResolvedValue('/home/user') }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const mockCreatedProfile = {
  id: 1, name: 'Test', mode: 'read-write', s3_endpoint: 'https://s3.test.com',
  s3_region: null, s3_bucket: 'bucket', extra_env: null, relative_path: null,
  temp_directory: null, s3_key_prefix: null, chunk_size_bytes: 10485760,
  is_active: true, created_at: '2024-01-01',
};

function setupInvoke({ encryptionKey = 'abc123def456' } = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'create_profile') return Promise.resolve({ profile: mockCreatedProfile, encryption_key: encryptionKey });
    if (cmd === 'get_active_profile') return Promise.resolve(mockCreatedProfile);
    if (cmd === 'list_profiles') return Promise.resolve([mockCreatedProfile]);
    return Promise.resolve(null);
  });
}

/** Fill the five required ProfileForm fields and submit. */
async function fillAndSubmit({ importKey = '' } = {}) {
  fireEvent.input(screen.getByLabelText(/profile name/i), { target: { value: 'Test' } });
  fireEvent.input(screen.getByLabelText(/endpoint url/i), { target: { value: 'https://s3.test.com' } });
  fireEvent.input(screen.getByLabelText(/^bucket$/i), { target: { value: 'bucket' } });
  fireEvent.input(screen.getByLabelText(/access key/i), { target: { value: 'AKID' } });
  fireEvent.input(screen.getByLabelText(/secret key/i), { target: { value: 'secret' } });
  if (importKey) {
    fireEvent.input(screen.getByLabelText(/encryption key/i), { target: { value: importKey } });
  }
  await fireEvent.click(screen.getByRole('button', { name: /^create profile$/i }));
}

beforeEach(() => {
  vi.clearAllMocks();
  setupInvoke();
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
    writable: true,
    configurable: true,
  });
});

// ── Initial render ─────────────────────────────────────────────────────────────

describe('Setup page — initial render', () => {
  it('shows the ProfileForm (not the key panel) on first load', () => {
    render(Page);
    expect(screen.getByRole('button', { name: /^create profile$/i })).toBeInTheDocument();
    expect(screen.queryByText(/save your encryption key/i)).not.toBeInTheDocument();
  });
});

// ── Key display ────────────────────────────────────────────────────────────────

describe('Setup page — key display after creation', () => {
  it('shows the encryption key panel after a successful create (no import key)', async () => {
    render(Page);
    await fillAndSubmit();
    await waitFor(() =>
      expect(screen.getByText(/save your encryption key/i)).toBeInTheDocument()
    );
    expect(screen.getByText('abc123def456')).toBeInTheDocument();
  });

  it('"Continue to App" button navigates to /files', async () => {
    render(Page);
    await fillAndSubmit();
    await waitFor(() => screen.getByText(/continue to app/i));
    await fireEvent.click(screen.getByRole('button', { name: /continue to app/i }));
    expect(mockGoto).toHaveBeenCalledWith('/files');
  });

  it('Copy button writes the key to the clipboard', async () => {
    render(Page);
    await fillAndSubmit();
    await waitFor(() => screen.getByRole('button', { name: /^copy$/i }));
    await fireEvent.click(screen.getByRole('button', { name: /^copy$/i }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('abc123def456');
  });
});

// ── Import key path ────────────────────────────────────────────────────────────

describe('Setup page — import encryption key', () => {
  it('navigates directly to /files when an import key is provided', async () => {
    render(Page);
    await fillAndSubmit({ importKey: 'a'.repeat(64) });
    await waitFor(() => expect(mockGoto).toHaveBeenCalledWith('/files'));
    expect(screen.queryByText(/save your encryption key/i)).not.toBeInTheDocument();
  });
});
