import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ProfileForm from './ProfileForm.svelte';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const noop = async () => {};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('ProfileForm — encryption key section (read-write)', () => {
  it('shows the generate-key warning by default', () => {
    render(ProfileForm, { onsubmit: noop });
    expect(screen.getByText(/new encryption key will be generated/i)).toBeInTheDocument();
  });

  it('shows the import checkbox', () => {
    render(ProfileForm, { onsubmit: noop });
    expect(screen.getByRole('checkbox', { name: /import existing encryption key/i })).toBeInTheDocument();
  });

  it('checking the import checkbox shows the key input and hides the warning', async () => {
    render(ProfileForm, { onsubmit: noop });
    await fireEvent.click(screen.getByRole('checkbox', { name: /import existing encryption key/i }));
    expect(screen.getByLabelText(/^encryption key$/i)).toBeInTheDocument();
    expect(screen.queryByText(/new encryption key will be generated/i)).not.toBeInTheDocument();
  });

  it('unchecking the import checkbox hides the key input and restores the warning', async () => {
    render(ProfileForm, { onsubmit: noop });
    const checkbox = screen.getByRole('checkbox', { name: /import existing encryption key/i });
    await fireEvent.click(checkbox);
    await fireEvent.click(checkbox);
    expect(screen.queryByLabelText(/^encryption key$/i)).not.toBeInTheDocument();
    expect(screen.getByText(/new encryption key will be generated/i)).toBeInTheDocument();
  });
});

describe('ProfileForm — encryption key section (read-only)', () => {
  it('shows the key input directly without a checkbox', () => {
    render(ProfileForm, { onsubmit: noop, initial: { mode: 'read-only' } });
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    expect(screen.getByLabelText(/^encryption key$/i)).toBeInTheDocument();
  });

  it('key input is marked required', () => {
    render(ProfileForm, { onsubmit: noop, initial: { mode: 'read-only' } });
    expect(screen.getByLabelText(/^encryption key$/i)).toBeRequired();
  });

  it('shows the read-only decryption hint', () => {
    render(ProfileForm, { onsubmit: noop, initial: { mode: 'read-only' } });
    expect(screen.getByText(/read-only profiles can only decrypt/i)).toBeInTheDocument();
  });

  it('does not show the generate-key warning', () => {
    render(ProfileForm, { onsubmit: noop, initial: { mode: 'read-only' } });
    expect(screen.queryByText(/new encryption key will be generated/i)).not.toBeInTheDocument();
  });

  it('switching to read-write restores the checkbox and warning', async () => {
    render(ProfileForm, { onsubmit: noop, initial: { mode: 'read-only' } });
    const select = screen.getByRole('combobox', { name: /mode/i });
    select.value = 'read-write';
    await fireEvent.change(select);
    expect(screen.getByRole('checkbox', { name: /import existing encryption key/i })).toBeInTheDocument();
    expect(screen.getByText(/new encryption key will be generated/i)).toBeInTheDocument();
  });
});

describe('ProfileForm — encryption key section (editing existing profile)', () => {
  it('hides the entire encryption key section', () => {
    render(ProfileForm, { onsubmit: noop, initial: { id: 1, name: 'existing' } });
    expect(screen.queryByText(/encryption key/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });
});

describe('ProfileForm — submission', () => {
  async function submitForm(container: HTMLElement) {
    await fireEvent.submit(container.querySelector('form')!);
  }

  it('passes null for import_encryption_key when read-write and not importing', async () => {
    const mockSubmit = vi.fn().mockResolvedValue(undefined);
    const { container } = render(ProfileForm, { onsubmit: mockSubmit });
    await submitForm(container);
    expect(mockSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ import_encryption_key: null, mode: 'read-write' })
    );
  });

  it('passes the key when read-write with import checked', async () => {
    const mockSubmit = vi.fn().mockResolvedValue(undefined);
    const { container } = render(ProfileForm, { onsubmit: mockSubmit });
    await fireEvent.click(screen.getByRole('checkbox', { name: /import existing encryption key/i }));
    await fireEvent.input(screen.getByLabelText(/^encryption key$/i), {
      target: { value: 'aabbcc' },
    });
    await submitForm(container);
    expect(mockSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ import_encryption_key: 'aabbcc' })
    );
  });

  it('passes the key as import_encryption_key when read-only', async () => {
    const mockSubmit = vi.fn().mockResolvedValue(undefined);
    const { container } = render(ProfileForm, { onsubmit: mockSubmit, initial: { mode: 'read-only' } });
    await fireEvent.input(screen.getByLabelText(/^encryption key$/i), {
      target: { value: 'deadbeef' },
    });
    await submitForm(container);
    expect(mockSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ import_encryption_key: 'deadbeef', mode: 'read-only' })
    );
  });

  it('passes null for import_encryption_key when read-only but key field is empty', async () => {
    const mockSubmit = vi.fn().mockResolvedValue(undefined);
    const { container } = render(ProfileForm, { onsubmit: mockSubmit, initial: { mode: 'read-only' } });
    await submitForm(container);
    expect(mockSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ import_encryption_key: null })
    );
  });
});
