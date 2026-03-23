import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { selectionStore } from '$lib/stores/selection.svelte';
import Page from './+page.svelte';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/api/path', () => ({ homeDir: vi.fn().mockResolvedValue('/home/user') }));

// Stub out the dynamically-imported modals so they don't fail the module graph.
vi.mock('$lib/components/BackupDirectoryModal.svelte', () => ({ default: { name: 'BackupDirectoryModal' } }));
vi.mock('$lib/components/RestoreModal.svelte', () => ({ default: { name: 'RestoreModal' } }));
vi.mock('$lib/components/VerifyIntegrityModal.svelte', () => ({ default: { name: 'VerifyIntegrityModal' } }));

const mockInvoke = vi.mocked(invoke);
const mockOpen = vi.mocked(open);

const mockFiles = [
  { id: 1, object_uuid: 'md5a', filename: 'alpha.txt', local_path: '/home/user/alpha.txt', file_size: 1024, original_md5: 'aabbcc112233', created_at: '2024-01-01' },
  { id: 2, object_uuid: 'md5b', filename: 'beta.pdf',  local_path: '/home/user/beta.pdf',  file_size: 2048, original_md5: 'ddeeff445566', created_at: '2024-01-02' },
];

function setupInvoke({ files = mockFiles } = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'list_files') return Promise.resolve(files);
    if (cmd === 'backup_file') return Promise.resolve('op-1');
    if (cmd === 'delete_backup_entries') return Promise.resolve(1);
    return Promise.resolve(null);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  selectionStore.clear();
  setupInvoke();
});

// ── File loading ──────────────────────────────────────────────────────────────

describe('Files page — loading', () => {
  it('calls list_files on mount with null search', async () => {
    render(Page);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: null }));
  });

  it('renders file paths from the response', async () => {
    render(Page);
    await waitFor(() => {
      expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument();
      expect(screen.getByText('/home/user/beta.pdf')).toBeInTheDocument();
    });
  });

  it('shows "No files found" when the response is empty', async () => {
    setupInvoke({ files: [] });
    render(Page);
    await waitFor(() => expect(screen.getByText(/no files found/i)).toBeInTheDocument());
  });
});

// ── Search ────────────────────────────────────────────────────────────────────

describe('Files page — search', () => {
  it('calls list_files with the search term after debounce', async () => {
    render(Page);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: null }));
    mockInvoke.mockClear();

    vi.useFakeTimers();
    const input = screen.getByPlaceholderText(/search files/i);
    fireEvent.input(input, { target: { value: 'alpha' } });
    vi.advanceTimersByTime(300);

    expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: 'alpha' });
    vi.useRealTimers();
  });

  it('does not call list_files immediately on input (debounced)', async () => {
    render(Page);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: null }));
    mockInvoke.mockClear();

    vi.useFakeTimers();
    const input = screen.getByPlaceholderText(/search files/i);
    fireEvent.input(input, { target: { value: 'beta' } });
    expect(mockInvoke).not.toHaveBeenCalledWith('list_files', { search: 'beta' });
    vi.useRealTimers();
  });

  it('passes null when the search term is cleared', async () => {
    render(Page);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: null }));
    mockInvoke.mockClear();

    vi.useFakeTimers();
    const input = screen.getByPlaceholderText(/search files/i);
    fireEvent.input(input, { target: { value: '' } });
    vi.advanceTimersByTime(300);

    expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: null });
    vi.useRealTimers();
  });
});

// ── Selection bar ─────────────────────────────────────────────────────────────

describe('Files page — selection bar', () => {
  it('does not show action buttons when nothing is selected', async () => {
    render(Page);
    await waitFor(() => expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: /^restore$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^verify$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^delete$/i })).not.toBeInTheDocument();
  });

  it('shows Restore and Verify buttons when items are selected', async () => {
    render(Page);
    await waitFor(() => expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument());
    selectionStore.toggle(1);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /^restore$/i })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: /^verify$/i })).toBeInTheDocument();
    });
  });

  it('shows the selection count in the page header', async () => {
    render(Page);
    await waitFor(() => expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument());
    selectionStore.toggle(1);
    selectionStore.toggle(2);
    await waitFor(() => expect(screen.getByText(/2 selected/i)).toBeInTheDocument());
  });
});

// ── Backup File ───────────────────────────────────────────────────────────────

describe('Files page — Backup File', () => {
  it('opens a file picker and calls backup_file with the chosen path', async () => {
    mockOpen.mockResolvedValueOnce('/home/user/newfile.txt');
    render(Page);
    await waitFor(() => expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument());

    await fireEvent.click(screen.getByRole('button', { name: /^backup file$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('backup_file', { filePath: '/home/user/newfile.txt' })
    );
  });

  it('does not call backup_file if the file picker is cancelled', async () => {
    mockOpen.mockResolvedValueOnce(null);
    render(Page);
    await waitFor(() => expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument());

    await fireEvent.click(screen.getByRole('button', { name: /^backup file$/i }));
    await waitFor(() => expect(mockOpen).toHaveBeenCalled());
    expect(mockInvoke).not.toHaveBeenCalledWith('backup_file', expect.anything());
  });
});

// ── Delete selected ───────────────────────────────────────────────────────────

describe('Files page — delete selected', () => {
  async function renderAndSelect() {
    render(Page);
    await waitFor(() => expect(screen.getByText('/home/user/alpha.txt')).toBeInTheDocument());
    selectionStore.toggle(1);
    selectionStore.toggle(2);
    await waitFor(() => expect(screen.getByRole('button', { name: /^delete$/i })).toBeInTheDocument());
  }

  it('shows a confirm modal before deleting', async () => {
    await renderAndSelect();
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    await waitFor(() => expect(screen.getByText(/delete backups/i)).toBeInTheDocument());
  });

  it('calls delete_backup_entries with selected ids on confirm', async () => {
    await renderAndSelect();
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^delete$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('delete_backup_entries', {
        backupEntryIds: expect.arrayContaining([1, 2]),
      })
    );
  });

  it('clears selection after successful delete', async () => {
    await renderAndSelect();
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^delete$/i }));
    await waitFor(() => expect(selectionStore.count).toBe(0));
  });

  it('cancelling the confirm modal does not call delete_backup_entries', async () => {
    await renderAndSelect();
    await fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    await waitFor(() => expect(screen.getByRole('button', { name: /cancel/i })).toBeInTheDocument());
    await fireEvent.click(screen.getByRole('button', { name: /cancel/i }));
    expect(mockInvoke).not.toHaveBeenCalledWith('delete_backup_entries', expect.anything());
  });
});
