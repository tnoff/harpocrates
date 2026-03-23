import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import { profileStore, type Profile } from '$lib/stores/profile.svelte';
import Page from './+page.svelte';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock('@tauri-apps/api/path', () => ({ homeDir: vi.fn().mockResolvedValue('/home/user') }));

const mockInvoke = vi.mocked(invoke);

const mockProfile: Profile = {
  id: 1, name: 'Work S3', mode: 'read-write', s3_endpoint: 'https://s3.example.com',
  s3_region: 'us-east-1', s3_bucket: 'work-bucket', extra_env: null, relative_path: null,
  temp_directory: null, s3_key_prefix: null, chunk_size_bytes: 10485760,
  is_active: true, created_at: '2024-01-01',
};

const DB_PATH = '/home/user/.local/share/harpocrates/harpocrates.db';

function setupInvoke(profiles: Profile[] = [mockProfile]) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'get_active_profile') return Promise.resolve(profiles.find((p) => p.is_active) ?? null);
    if (cmd === 'list_profiles') return Promise.resolve(profiles);
    if (cmd === 'get_throttle_limits') return Promise.resolve({ upload_bps: 0, download_bps: 0 });
    if (cmd === 'get_config') return Promise.resolve({ database_path: DB_PATH });
    if (cmd === 'get_profile_credentials') return Promise.resolve({ s3_access_key: 'KEY', s3_secret_key: 'SECRET' });
    if (cmd === 'delete_profile') return Promise.resolve(null);
    if (cmd === 'set_throttle_limits') return Promise.resolve(null);
    if (cmd === 'set_database_path') return Promise.resolve(null);
    if (cmd === 'export_profile_config') return Promise.resolve(null);
    if (cmd === 'import_profile_config') return Promise.resolve(null);
    if (cmd === 'update_profile') return Promise.resolve(null);
    if (cmd === 'create_profile') return Promise.resolve(null);
    return Promise.resolve(null);
  });
}

beforeEach(async () => {
  vi.clearAllMocks();
  setupInvoke();
  await profileStore.load();
});

// ── Profile list ───────────────────────────────────────────────────────────────

describe('Settings page — profile list', () => {
  it('shows each profile name', async () => {
    render(Page);
    await waitFor(() => expect(screen.getByText('Work S3')).toBeInTheDocument());
  });

  it('shows the Active badge on the active profile', async () => {
    render(Page);
    await waitFor(() => expect(screen.getByText('Active')).toBeInTheDocument());
  });

  it('shows the endpoint and bucket in the profile meta line', async () => {
    render(Page);
    await waitFor(() => expect(screen.getByText(/work-bucket/i)).toBeInTheDocument());
  });

  it('shows all profiles when there are multiple', async () => {
    const second: Profile = { ...mockProfile, id: 2, name: 'Personal B2', is_active: false };
    setupInvoke([mockProfile, second]);
    await profileStore.load();
    render(Page);
    await waitFor(() => {
      expect(screen.getByText('Work S3')).toBeInTheDocument();
      expect(screen.getByText('Personal B2')).toBeInTheDocument();
    });
  });
});

// ── Add / Edit profile modals ─────────────────────────────────────────────────

describe('Settings page — add / edit modals', () => {
  it('clicking "Add Profile" opens the modal with "New Profile" heading', async () => {
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /^add profile$/i }));
    await waitFor(() =>
      expect(screen.getByRole('dialog')).toBeInTheDocument()
    );
    expect(within(screen.getByRole('dialog')).getByText(/new profile/i)).toBeInTheDocument();
  });

  it('clicking "Edit" fetches credentials and opens the modal with "Edit Profile" heading', async () => {
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /^edit$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('get_profile_credentials', { profileId: 1 })
    );
    await waitFor(() =>
      expect(within(screen.getByRole('dialog')).getByText(/edit profile/i)).toBeInTheDocument()
    );
  });

  it('closing the modal via Cancel dismisses it', async () => {
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /^add profile$/i }));
    await waitFor(() => screen.getByRole('dialog'));
    await fireEvent.click(screen.getByRole('button', { name: /^cancel$/i }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    );
  });
});

// ── Delete profile ─────────────────────────────────────────────────────────────

describe('Settings page — delete profile', () => {
  it('clicking Delete opens the confirm modal', async () => {
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeInTheDocument());
    expect(within(screen.getByRole('dialog')).getByText(/delete profile/i)).toBeInTheDocument();
  });

  it('confirming delete calls delete_profile with the profile id', async () => {
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^delete$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('delete_profile', { profileId: 1 })
    );
  });

  it('cancelling the delete modal does not call delete_profile', async () => {
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /cancel/i }));
    expect(mockInvoke).not.toHaveBeenCalledWith('delete_profile', expect.anything());
  });
});

// ── Bandwidth throttle ─────────────────────────────────────────────────────────

describe('Settings page — bandwidth throttle', () => {
  it('loads throttle limits on mount', async () => {
    render(Page);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('get_throttle_limits')
    );
  });

  it('Apply calls set_throttle_limits with the correct byte values', async () => {
    render(Page);
    await waitFor(() => screen.getByLabelText(/upload limit/i));

    const upload = screen.getByLabelText(/upload limit/i);
    const download = screen.getByLabelText(/download limit/i);
    await fireEvent.input(upload, { target: { value: '512' } });
    await fireEvent.input(download, { target: { value: '1024' } });
    await fireEvent.click(screen.getByRole('button', { name: /^apply$/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_throttle_limits', {
        uploadBps: 512 * 1024,
        downloadBps: 1024 * 1024,
      })
    );
  });

  it('setting both limits to 0 sends 0 for unlimited', async () => {
    render(Page);
    await waitFor(() => screen.getByLabelText(/upload limit/i));
    await fireEvent.click(screen.getByRole('button', { name: /^apply$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_throttle_limits', {
        uploadBps: 0,
        downloadBps: 0,
      })
    );
  });
});

// ── Database path ──────────────────────────────────────────────────────────────

describe('Settings page — database path', () => {
  it('loads and displays the current database path', async () => {
    render(Page);
    await waitFor(() =>
      expect(screen.getByDisplayValue(DB_PATH)).toBeInTheDocument()
    );
  });

  it('Apply button is hidden in the Database section when path matches current', async () => {
    render(Page);
    await waitFor(() => screen.getByDisplayValue(DB_PATH));
    const dbSection = screen.getByText('Database', { selector: 'h3' }).closest('section')!;
    expect(within(dbSection).queryByRole('button', { name: /^apply$/i })).not.toBeInTheDocument();
  });

  it('Apply button appears when path is changed and calls set_database_path', async () => {
    render(Page);
    await waitFor(() => screen.getByDisplayValue(DB_PATH));
    const dbSection = screen.getByText('Database', { selector: 'h3' }).closest('section')!;
    const input = screen.getByDisplayValue(DB_PATH);
    await fireEvent.input(input, { target: { value: '/new/path/vault.db' } });
    await waitFor(() =>
      expect(within(dbSection).getByRole('button', { name: /^apply$/i })).toBeInTheDocument()
    );
    await fireEvent.click(within(dbSection).getByRole('button', { name: /^apply$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_database_path', {
        newPath: '/new/path/vault.db',
        copyExisting: true,
      })
    );
  });
});

// ── Import profile ─────────────────────────────────────────────────────────────

describe('Settings page — import profile', () => {
  it('clicking "Import Profile" opens the file picker', async () => {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const mockOpen = vi.mocked(open);
    mockOpen.mockResolvedValueOnce(null); // cancel the picker
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /import profile/i }));
    await waitFor(() => expect(mockOpen).toHaveBeenCalled());
  });

  it('selecting a file opens the import modal', async () => {
    const { open } = await import('@tauri-apps/plugin-dialog');
    vi.mocked(open).mockResolvedValueOnce('/home/user/profile.json');
    render(Page);
    await waitFor(() => screen.getByText('Work S3'));
    await fireEvent.click(screen.getByRole('button', { name: /import profile/i }));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeInTheDocument());
    expect(within(screen.getByRole('dialog')).getByText(/import profile/i)).toBeInTheDocument();
  });
});
