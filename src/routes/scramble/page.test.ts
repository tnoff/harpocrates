import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { invoke } from '@tauri-apps/api/core';
import { selectionStore } from '$lib/stores/selection.svelte';
import Page from './+page.svelte';

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  vi.clearAllMocks();
  selectionStore.clear();
  mockInvoke.mockResolvedValue('op-1');
});

// ── Button state ───────────────────────────────────────────────────────────────

describe('Scramble page — button state', () => {
  it('Scramble button is disabled when no files are selected (selected-only mode)', () => {
    render(Page);
    expect(screen.getByRole('button', { name: /^scramble$/i })).toBeDisabled();
  });

  it('shows the selection count in the radio label', () => {
    selectionStore.toggle(1);
    selectionStore.toggle(2);
    render(Page);
    expect(screen.getByText(/selected files only \(2 selected\)/i)).toBeInTheDocument();
  });

  it('Scramble button is enabled after switching to "All files"', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('radio', { name: /all files/i }));
    expect(screen.getByRole('button', { name: /^scramble$/i })).not.toBeDisabled();
  });

  it('Scramble button becomes enabled when files are selected', async () => {
    render(Page);
    selectionStore.toggle(5);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^scramble$/i })).not.toBeDisabled()
    );
  });
});

// ── Confirm modal ──────────────────────────────────────────────────────────────

describe('Scramble page — confirm modal', () => {
  async function renderWithSelection(...ids: number[]) {
    ids.forEach((id) => selectionStore.toggle(id));
    render(Page);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^scramble$/i })).not.toBeDisabled()
    );
  }

  it('clicking Scramble opens the confirm modal', async () => {
    await renderWithSelection(1);
    await fireEvent.click(screen.getByRole('button', { name: /^scramble$/i }));
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('calls scramble with selected ids and scrambleAll=false', async () => {
    await renderWithSelection(3, 7);
    await fireEvent.click(screen.getByRole('button', { name: /^scramble$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^scramble$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('scramble', {
        backupEntryIds: expect.arrayContaining([3, 7]),
        scrambleAll: false,
      })
    );
    const call = mockInvoke.mock.calls.find((c) => c[0] === 'scramble')!;
    expect((call[1] as { backupEntryIds: number[] }).backupEntryIds).toHaveLength(2);
  });

  it('calls scramble with scrambleAll=true and empty ids in "All files" mode', async () => {
    render(Page);
    await fireEvent.click(screen.getByRole('radio', { name: /all files/i }));
    await fireEvent.click(screen.getByRole('button', { name: /^scramble$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /^scramble$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('scramble', {
        backupEntryIds: [],
        scrambleAll: true,
      })
    );
  });

  it('cancelling the confirm modal does not call invoke("scramble")', async () => {
    await renderWithSelection(1);
    await fireEvent.click(screen.getByRole('button', { name: /^scramble$/i }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: /cancel/i }));
    expect(mockInvoke).not.toHaveBeenCalledWith('scramble', expect.anything());
  });
});
