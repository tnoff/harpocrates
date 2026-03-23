// Stub for $app/navigation used in vitest. Tests override these with vi.mock().
export const goto = () => Promise.resolve();
export const beforeNavigate = () => {};
export const afterNavigate = () => {};
export const onNavigate = () => {};
export const invalidate = () => Promise.resolve();
export const invalidateAll = () => Promise.resolve();
export const preloadCode = () => Promise.resolve();
export const preloadData = () => Promise.resolve();
export const pushState = () => {};
export const replaceState = () => {};
