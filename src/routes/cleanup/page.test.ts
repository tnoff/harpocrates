import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import Page from './+page.svelte';

const mockInvoke = vi.mocked(invoke);

const mockLocalOrphans = [
  { local_file_id: 1, file_entry_id: 10, local_path: '/home/user/deleted.txt' },
  { local_file_id: 2, file_entry_id: 11, local_path: '/home/user/gone.pdf' },
  { local_file_id: 3, file_entry_id: 12, local_path: '/home/user/missing.jpg' },
];

const mockS3Orphans = [
  { key: 'prefix/c/aabbcc', size: 1024 },
  { key: 'prefix/c/ddeeff', size: 2048 },
];

function setupInvoke({ localOrphans = mockLocalOrphans, s3Orphans = mockS3Orphans } = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'scan_orphaned_local_entries') return Promise.resolve(localOrphans);
    if (cmd === 'scan_orphaned_s3_objects') return Promise.resolve(s3Orphans);
    if (cmd === 'cleanup_orphaned_local_entries') return Promise.resolve('op-1');
    if (cmd === 'cleanup_orphaned_s3_objects') return Promise.resolve('op-2');
    return Promise.resolve(null);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  setupInvoke();
});

// ── Tab switching ─────────────────────────────────────────────────────────────

describe('Cleanup page — tabs', () => {
  it('defaults to the local orphans tab', () => {
    render(Page);
    expect(screen.getByRole('button', { name: /scan for orphaned entries/i })).toBeInTheDocument();
  });

  it('switching to the S3 tab shows the S3 scan button', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /orphaned s3 objects/i }));
    expect(screen.getByRole('button', { name: /scan for orphaned s3 objects/i })).toBeInTheDocument();
  });

  it('local state is independent from S3 state', async () => {
    render(Page);
    // Scan local
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => expect(screen.getByText('/home/user/deleted.txt')).toBeInTheDocument());

    // Switch to S3 tab — local results should not appear
    await fireEvent.click(screen.getByRole('button', { name: /orphaned s3 objects/i }));
    expect(screen.queryByText('/home/user/deleted.txt')).not.toBeInTheDocument();
  });
});

// ── Local orphans: scan ───────────────────────────────────────────────────────

describe('Cleanup page — local scan', () => {
  it('calls scan_orphaned_local_entries on button click', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('scan_orphaned_local_entries'));
  });

  it('shows orphan rows after scan', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => {
      expect(screen.getByText('/home/user/deleted.txt')).toBeInTheDocument();
      expect(screen.getByText('/home/user/gone.pdf')).toBeInTheDocument();
      expect(screen.getByText('/home/user/missing.jpg')).toBeInTheDocument();
    });
  });

  it('displays the correct file_entry_id in the Entry ID column', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => expect(screen.getByText('10')).toBeInTheDocument());
    expect(screen.getByText('11')).toBeInTheDocument();
    expect(screen.getByText('12')).toBeInTheDocument();
  });

  it('shows empty message when no orphans found', async () => {
    setupInvoke({ localOrphans: [] });
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() =>
      expect(screen.getByText(/no orphaned local entries found/i)).toBeInTheDocument()
    );
  });

  it('pre-selects all orphans after scan', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => expect(screen.getByText('/home/user/deleted.txt')).toBeInTheDocument());
    // All 3 should be selected → delete button shows count 3
    expect(screen.getByRole('button', { name: /delete 3 entries/i })).toBeInTheDocument();
  });
});

// ── Local orphans: selection ──────────────────────────────────────────────────

describe('Cleanup page — local selection', () => {
  async function renderAfterScan() {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => expect(screen.getByText('/home/user/deleted.txt')).toBeInTheDocument());
  }

  it('unchecking a row removes it from the selection count', async () => {
    await renderAfterScan();
    const checkboxes = screen.getAllByRole('checkbox');
    // index 0 = header, 1-3 = rows
    await fireEvent.click(checkboxes[1]);
    expect(screen.getByRole('button', { name: /delete 2 entries/i })).toBeInTheDocument();
  });

  it('header checkbox deselects all', async () => {
    await renderAfterScan();
    const header = screen.getAllByRole('checkbox')[0];
    await fireEvent.click(header); // all → none
    expect(screen.getByRole('button', { name: /delete 0 entries/i })).toBeInTheDocument();
  });

  it('header checkbox re-selects all when none are selected', async () => {
    await renderAfterScan();
    const header = screen.getAllByRole('checkbox')[0];
    await fireEvent.click(header); // deselect all
    await fireEvent.click(header); // re-select all
    expect(screen.getByRole('button', { name: /delete 3 entries/i })).toBeInTheDocument();
  });

  it('delete button is disabled when no entries selected', async () => {
    await renderAfterScan();
    const header = screen.getAllByRole('checkbox')[0];
    await fireEvent.click(header); // deselect all
    expect(screen.getByRole('button', { name: /delete 0 entries/i })).toBeDisabled();
  });
});

// ── Local orphans: dry run & cleanup ─────────────────────────────────────────

describe('Cleanup page — local cleanup', () => {
  async function renderAfterScan() {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned entries/i }));
    await waitFor(() => expect(screen.getByText('/home/user/deleted.txt')).toBeInTheDocument());
  }

  it('delete button label includes (dry run) by default', async () => {
    await renderAfterScan();
    expect(screen.getByRole('button', { name: /dry run/i })).toBeInTheDocument();
  });

  it('unchecking dry run removes the (dry run) label', async () => {
    await renderAfterScan();
    const dryRunCheckbox = screen.getByLabelText(/dry run/i);
    await fireEvent.click(dryRunCheckbox);
    expect(screen.queryByText(/dry run/i, { selector: 'button' })).not.toBeInTheDocument();
  });

  it('calls cleanup_orphaned_local_entries with selected ids and dryRun=true by default', async () => {
    await renderAfterScan();
    // Deselect one entry
    await fireEvent.click(screen.getAllByRole('checkbox')[3]); // uncheck row 3
    await fireEvent.click(screen.getByRole('button', { name: /delete 2 entries/i }));
    // Confirm modal appears — button label is "Dry Run" because dry run is on by default
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^dry run$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('cleanup_orphaned_local_entries', {
        localFileIds: expect.arrayContaining([1, 2]),
        deleteS3: false,
        dryRun: true,
      })
    );
    const call = mockInvoke.mock.calls.find((c) => c[0] === 'cleanup_orphaned_local_entries')!;
    expect((call[1] as { localFileIds: number[] }).localFileIds).toHaveLength(2);
  });
});

// ── S3 orphans: scan & selection ─────────────────────────────────────────────

describe('Cleanup page — S3 scan', () => {
  async function renderOnS3Tab() {
    render(Page);
    await fireEvent.click(screen.getByRole('button', { name: /orphaned s3 objects/i }));
  }

  it('calls scan_orphaned_s3_objects on button click', async () => {
    await renderOnS3Tab();
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned s3 objects/i }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('scan_orphaned_s3_objects'));
  });

  it('shows S3 orphan keys and sizes after scan', async () => {
    await renderOnS3Tab();
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned s3 objects/i }));
    await waitFor(() => {
      expect(screen.getByText('prefix/c/aabbcc')).toBeInTheDocument();
      expect(screen.getByText('prefix/c/ddeeff')).toBeInTheDocument();
    });
  });

  it('pre-selects all S3 orphans after scan', async () => {
    await renderOnS3Tab();
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned s3 objects/i }));
    await waitFor(() => expect(screen.getByText('prefix/c/aabbcc')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: /delete 2 objects/i })).toBeInTheDocument();
  });

  it('calls cleanup_orphaned_s3_objects with selected keys', async () => {
    await renderOnS3Tab();
    await fireEvent.click(screen.getByRole('button', { name: /scan for orphaned s3 objects/i }));
    await waitFor(() => expect(screen.getByText('prefix/c/aabbcc')).toBeInTheDocument());
    await fireEvent.click(screen.getByRole('button', { name: /delete 2 objects/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^dry run$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('cleanup_orphaned_s3_objects', {
        objectKeys: expect.arrayContaining(['prefix/c/aabbcc', 'prefix/c/ddeeff']),
        dryRun: true,
      })
    );
  });
});
