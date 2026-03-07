import '@testing-library/jest-dom';
import { vi } from 'vitest';

// Default Tauri API mocks so any test that imports a Tauri-dependent module
// doesn't throw on import. Tests that need specific behaviour override these.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
