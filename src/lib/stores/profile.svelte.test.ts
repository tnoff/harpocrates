import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Profile } from './profile.svelte';

const mockProfile = (overrides: Partial<Profile> = {}): Profile => ({
  id: 1,
  name: 'test',
  mode: 'read-write',
  s3_endpoint: 'https://s3.example.com',
  s3_region: null,
  s3_bucket: 'my-bucket',
  extra_env: null,
  relative_path: null,
  temp_directory: null,
  is_active: true,
  created_at: '2024-01-01T00:00:00Z',
  ...overrides,
});

let mockInvoke: ReturnType<typeof vi.fn>;

beforeEach(async () => {
  vi.resetModules();
  mockInvoke = vi.fn();
  vi.doMock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
});

async function loadStore() {
  const mod = await import('./profile.svelte');
  return mod.profileStore;
}

describe('profileStore', () => {
  it('isReadOnly is false when mode is read-write', async () => {
    mockInvoke.mockResolvedValue(mockProfile({ mode: 'read-write' }));
    const store = await loadStore();
    await store.load();
    expect(store.isReadOnly).toBe(false);
  });

  it('isReadOnly is true when mode is read-only', async () => {
    mockInvoke.mockResolvedValue(mockProfile({ mode: 'read-only' }));
    const store = await loadStore();
    await store.load();
    expect(store.isReadOnly).toBe(true);
  });

  it('isReadOnly is false when no active profile', async () => {
    mockInvoke
      .mockResolvedValueOnce(null)  // get_active_profile
      .mockResolvedValueOnce([]);   // list_profiles
    const store = await loadStore();
    await store.load();
    expect(store.isReadOnly).toBe(false);
  });

  it('loading starts true, false after load()', async () => {
    mockInvoke
      .mockResolvedValueOnce(mockProfile())
      .mockResolvedValueOnce([mockProfile()]);
    const store = await loadStore();
    expect(store.loading).toBe(true);
    await store.load();
    expect(store.loading).toBe(false);
  });

  it('load() sets active and profiles', async () => {
    const profile = mockProfile();
    mockInvoke
      .mockResolvedValueOnce(profile)     // get_active_profile
      .mockResolvedValueOnce([profile]);  // list_profiles
    const store = await loadStore();
    await store.load();
    expect(store.active).toEqual(profile);
    expect(store.profiles).toEqual([profile]);
  });

  it('load() sets loading to false even on error', async () => {
    mockInvoke.mockRejectedValue(new Error('network error'));
    const store = await loadStore();
    await store.load().catch(() => {});
    expect(store.loading).toBe(false);
  });

  it('switchProfile() calls switch_profile with correct id', async () => {
    const profile = mockProfile({ id: 42 });
    mockInvoke
      .mockResolvedValueOnce(profile)   // switch_profile
      .mockResolvedValueOnce(profile)   // get_active_profile (from load)
      .mockResolvedValueOnce([profile]); // list_profiles (from load)
    const store = await loadStore();
    await store.switchProfile(42);
    expect(mockInvoke).toHaveBeenCalledWith('switch_profile', { profileId: 42 });
  });

  it('switchProfile() updates active profile', async () => {
    const profile = mockProfile({ id: 42, name: 'new-profile' });
    mockInvoke
      .mockResolvedValueOnce(profile)   // switch_profile
      .mockResolvedValueOnce(profile)   // get_active_profile (from load)
      .mockResolvedValueOnce([profile]); // list_profiles (from load)
    const store = await loadStore();
    await store.switchProfile(42);
    expect(store.active?.id).toBe(42);
  });
});
