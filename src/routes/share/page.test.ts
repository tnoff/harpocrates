import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import { selectionStore } from '$lib/stores/selection.svelte';
import Page from './+page.svelte';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const mockFiles = [
  { id: 1, object_uuid: 'uuid-1', filename: 'alpha.txt', local_path: '/home/user/Music/Some Artist/alpha.txt', file_size: 1024,   original_md5: 'md5a', created_at: '2024-01-01' },
  { id: 2, object_uuid: 'uuid-2', filename: 'beta.txt',  local_path: '/home/user/Music/Some Artist/beta.txt',  file_size: 2048,   original_md5: 'md5b', created_at: '2024-01-02' },
  { id: 3, object_uuid: 'uuid-3', filename: 'gamma.txt', local_path: '/home/user/Documents/gamma.txt',         file_size: 512,    original_md5: 'md5c', created_at: '2024-01-03' },
];

function setupInvoke({ files = mockFiles, createUuid = 'test-uuid-1234' } = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'list_files') return Promise.resolve(files);
    if (cmd === 'create_share_manifest') return Promise.resolve(createUuid);
    if (cmd === 'list_share_manifests_cmd') return Promise.resolve([]);
    return Promise.resolve(null);
  });
}

async function renderAndWait() {
  const result = render(Page);
  await waitFor(() => expect(screen.getByText('alpha.txt')).toBeInTheDocument());
  return result;
}

beforeEach(() => {
  vi.clearAllMocks();
  selectionStore.clear();
  setupInvoke();
});

describe('Share page — file picker loading', () => {
  it('calls list_files on mount', async () => {
    render(Page);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_files', { search: null }));
  });

  it('shows all files in the picker', async () => {
    await renderAndWait();
    expect(screen.getByText('alpha.txt')).toBeInTheDocument();
    expect(screen.getByText('beta.txt')).toBeInTheDocument();
    expect(screen.getByText('gamma.txt')).toBeInTheDocument();
  });

  it('shows empty message when bucket has no files', async () => {
    setupInvoke({ files: [] });
    render(Page);
    await waitFor(() => expect(screen.getByText(/no files in bucket/i)).toBeInTheDocument());
  });
});

describe('Share page — initial selection', () => {
  it('starts with 0 selected when selectionStore is empty', async () => {
    await renderAndWait();
    expect(screen.getByText('0 selected')).toBeInTheDocument();
  });

  it('pre-selects files that were already selected in selectionStore', async () => {
    selectionStore.toggle(1);
    selectionStore.toggle(3);
    render(Page);
    await waitFor(() => expect(screen.getByText('2 selected')).toBeInTheDocument());
  });
});

describe('Share page — row / checkbox selection', () => {
  it('clicking a row selects it', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('row')[1]); // first data row
    expect(screen.getByText('1 selected')).toBeInTheDocument();
  });

  it('clicking a selected row deselects it', async () => {
    await renderAndWait();
    const row = screen.getAllByRole('row')[1];
    await fireEvent.click(row);
    await fireEvent.click(row);
    expect(screen.getByText('0 selected')).toBeInTheDocument();
  });

  it('checking a row checkbox selects it', async () => {
    await renderAndWait();
    const checkboxes = screen.getAllByRole('checkbox');
    await fireEvent.click(checkboxes[1]); // first row checkbox (index 0 is header)
    expect(screen.getByText('1 selected')).toBeInTheDocument();
  });

  it('multiple rows can be selected independently', async () => {
    await renderAndWait();
    const rows = screen.getAllByRole('row');
    await fireEvent.click(rows[1]);
    await fireEvent.click(rows[3]);
    expect(screen.getByText('2 selected')).toBeInTheDocument();
  });
});

describe('Share page — select all', () => {
  it('header checkbox selects all visible files', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('checkbox')[0]);
    expect(screen.getByText('3 selected')).toBeInTheDocument();
  });

  it('header checkbox deselects all when all are already selected', async () => {
    await renderAndWait();
    const header = screen.getAllByRole('checkbox')[0];
    await fireEvent.click(header); // select all
    await fireEvent.click(header); // deselect all
    expect(screen.getByText('0 selected')).toBeInTheDocument();
  });
});

describe('Share page — search filtering', () => {
  it('filters the displayed files by search term', async () => {
    await renderAndWait();
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'alpha';
    await fireEvent.input(search);
    expect(screen.getByText('alpha.txt')).toBeInTheDocument();
    expect(screen.queryByText('beta.txt')).not.toBeInTheDocument();
    expect(screen.queryByText('gamma.txt')).not.toBeInTheDocument();
  });

  it('shows all files when search is cleared', async () => {
    await renderAndWait();
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'alpha';
    await fireEvent.input(search);
    search.value = '';
    await fireEvent.input(search);
    expect(screen.getByText('beta.txt')).toBeInTheDocument();
    expect(screen.getByText('gamma.txt')).toBeInTheDocument();
  });

  it('select all only selects filtered files', async () => {
    await renderAndWait();
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'alpha';
    await fireEvent.input(search);
    await fireEvent.click(screen.getAllByRole('checkbox')[0]);
    expect(screen.getByText('1 selected')).toBeInTheDocument();
  });

  it('deselect all only deselects filtered files, leaving others untouched', async () => {
    await renderAndWait();
    // Select all 3 first
    await fireEvent.click(screen.getAllByRole('checkbox')[0]);
    expect(screen.getByText('3 selected')).toBeInTheDocument();
    // Filter to alpha only and deselect all
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'alpha';
    await fireEvent.input(search);
    await fireEvent.click(screen.getAllByRole('checkbox')[0]);
    // Clear search — beta and gamma should still be selected
    search.value = '';
    await fireEvent.input(search);
    expect(screen.getByText('2 selected')).toBeInTheDocument();
  });
});

describe('Share page — search by local path', () => {
  it('matches files by directory name in local_path', async () => {
    await renderAndWait();
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'Some Artist';
    await fireEvent.input(search);
    expect(screen.getByText('alpha.txt')).toBeInTheDocument();
    expect(screen.getByText('beta.txt')).toBeInTheDocument();
    expect(screen.queryByText('gamma.txt')).not.toBeInTheDocument();
  });

  it('matches files by partial path segment', async () => {
    await renderAndWait();
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'Documents';
    await fireEvent.input(search);
    expect(screen.queryByText('alpha.txt')).not.toBeInTheDocument();
    expect(screen.queryByText('beta.txt')).not.toBeInTheDocument();
    expect(screen.getByText('gamma.txt')).toBeInTheDocument();
  });

  it('path search is case-insensitive', async () => {
    await renderAndWait();
    const search = screen.getByPlaceholderText(/search files/i);
    search.value = 'some artist';
    await fireEvent.input(search);
    expect(screen.getByText('alpha.txt')).toBeInTheDocument();
    expect(screen.getByText('beta.txt')).toBeInTheDocument();
  });
});

describe('Share page — create button state', () => {
  it('is disabled when no files are selected', async () => {
    await renderAndWait();
    expect(screen.getByRole('button', { name: /create share/i })).toBeDisabled();
  });

  it('is enabled once at least one file is selected', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('row')[1]);
    expect(screen.getByRole('button', { name: /create share/i })).not.toBeDisabled();
  });

  it('shows singular "file" for 1 selected', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('row')[1]);
    expect(screen.getByRole('button', { name: /create share/i })).toHaveTextContent('Create Share (1 file)');
  });

  it('shows plural "files" for multiple selected', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('checkbox')[0]); // select all 3
    expect(screen.getByRole('button', { name: /create share/i })).toHaveTextContent('Create Share (3 files)');
  });
});

describe('Share page — create manifest submission', () => {
  it('calls create_share_manifest with the selected backupEntryIds', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('row')[1]); // id 1
    await fireEvent.click(screen.getAllByRole('row')[3]); // id 3
    await fireEvent.click(screen.getByRole('button', { name: /create share/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('create_share_manifest', {
        backupEntryIds: expect.arrayContaining([1, 3]),
        label: null,
      })
    );
    // Only 2 ids should be sent
    const call = mockInvoke.mock.calls.find(c => c[0] === 'create_share_manifest')!;
    expect((call[1] as { backupEntryIds: number[] }).backupEntryIds).toHaveLength(2);
  });

  it('passes the label when provided', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('checkbox')[0]); // select all
    const labelInput = screen.getByLabelText(/label/i);
    labelInput.value = 'my share';
    await fireEvent.input(labelInput);
    await fireEvent.click(screen.getByRole('button', { name: /create share/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('create_share_manifest', expect.objectContaining({ label: 'my share' }))
    );
  });

  it('passes null label when label field is empty', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('checkbox')[0]);
    await fireEvent.click(screen.getByRole('button', { name: /create share/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('create_share_manifest', expect.objectContaining({ label: null }))
    );
  });

  it('shows the returned UUID after successful create', async () => {
    await renderAndWait();
    await fireEvent.click(screen.getAllByRole('checkbox')[0]);
    await fireEvent.click(screen.getByRole('button', { name: /create share/i }));
    await waitFor(() => expect(screen.getByText('test-uuid-1234')).toBeInTheDocument());
  });
});
