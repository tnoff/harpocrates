# Frontend Testing Plan

## Stack

| Package | Role |
|---------|------|
| `vitest` | Test runner (Vite-native, no separate config overhead) |
| `@sveltejs/vite-plugin-svelte` | Already installed — needed in vitest config to compile `.svelte.ts` rune files |
| `@testing-library/svelte` | Component rendering + querying |
| `@testing-library/jest-dom` | Extended DOM matchers (`toBeVisible`, `toHaveTextContent`, etc.) |
| `happy-dom` | Lightweight DOM environment (faster than jsdom for unit tests) |

### vitest config

Create `vitest.config.ts` (separate from `vite.config.js` to keep dev-server options out of tests):

```ts
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { resolve } from 'path';

export default defineConfig({
  plugins: [svelte({ hot: false })],
  resolve: {
    alias: { $lib: resolve('./src/lib') },
  },
  test: {
    environment: 'happy-dom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['./src/**/*.test.ts'],
  },
});
```

### Global setup (`src/test/setup.ts`)

- Import `@testing-library/jest-dom` for extended matchers
- Provide default Tauri API mocks so any test that imports a Tauri-dependent module doesn't throw on import

### Tauri API mocking strategy

The app uses two Tauri entry points that need mocking:

- `@tauri-apps/api/core` → `invoke` (command calls)
- `@tauri-apps/api/event` → `listen` (event subscriptions)

Pattern used in store tests that need event simulation:

```ts
// Capture callbacks registered by the store's IIFE
const listeners: Record<string, (e: { payload: unknown }) => void> = {};

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name, cb) => {
    listeners[name] = cb;
    return Promise.resolve(() => {}); // unlisten fn
  }),
}));

// Helper to fire a fake backend event
function emit(name: string, payload: unknown) {
  listeners[name]?.({ payload });
}
```

Because `operationsStore` registers listeners in a module-level async IIFE, each test that needs a clean state should use `vi.resetModules()` + dynamic `import()` in `beforeEach`, then `await` a tick to let the IIFE complete.

---

## Test files

### 1. `src/lib/stores/selection.svelte.test.ts`

**No mocking required** — zero Tauri dependencies.

Reset between tests: call `selectionStore.clear()` in `beforeEach`.

| Test | What it covers |
|------|---------------|
| starts empty | initial `count === 0`, `hasAny === false` |
| toggle adds an id | `toggle(1)` → `has(1) === true`, `count === 1` |
| toggle removes an existing id | two `toggle(1)` calls → `count === 0` |
| toggle multiple ids independently | toggle 1, 2, 3 → `count === 3` |
| selectAll replaces entire selection | pre-select 1, `selectAll([2,3])` → `has(1) === false`, `count === 2` |
| selectAll with empty array clears | `selectAll([])` → `count === 0` |
| clear empties the set | select several, `clear()` → `count === 0` |
| array returns all ids | `selectAll([10,20])` → `array` contains both |
| has returns false for missing id | `has(99) === false` on empty store |
| count stays accurate across operations | sequence of toggle/clear/selectAll |

---

### 2. `src/lib/stores/toast.svelte.test.ts`

**No mocking required** — zero Tauri dependencies.

Reset between tests: dismiss all `toast.items` in `beforeEach`. Use `vi.useFakeTimers()` / `vi.useRealTimers()` around timer-sensitive tests.

| Test | What it covers |
|------|---------------|
| `success()` creates a success toast | type, message |
| `error()` creates an error toast | type |
| `warning()` creates a warning toast | type |
| `info()` creates an info toast | type |
| each toast gets a unique id | three adds → three distinct ids |
| `dismiss(id)` removes only that toast | two toasts, dismiss first, second remains |
| success auto-dismisses after 3500 ms | `vi.advanceTimersByTime(3499)` → still present; `+1ms` → gone |
| error auto-dismisses after 6000 ms | still present at 5999 ms |
| warning auto-dismisses after 5000 ms | still present at 4999 ms |
| multiple toasts independent timers | success + error, advance 3500 ms → only error remains |
| `add` with custom duration | `add('info', msg, 1000)` → gone after 1000 ms |

---

### 3. `src/lib/stores/profile.svelte.test.ts`

**Requires `invoke` mock.**

Reset: `vi.resetModules()` + dynamic import in `beforeEach` for clean state.

| Test | What it covers |
|------|---------------|
| `isReadOnly` false when mode is `read-write` | mock `get_active_profile` returning `mode: 'read-write'` |
| `isReadOnly` true when mode is `read-only` | mock returning `mode: 'read-only'` |
| `isReadOnly` false when no active profile | mock returning `null` |
| `loading` starts true, false after `load()` | check state before and after |
| `load()` sets `active` and `profiles` | verify both invoke calls, check state |
| `load()` sets `loading: false` even on error | mock invoke rejection, verify finally block |
| `switchProfile(id)` updates `activeProfile` immediately | verify optimistic update |
| `switchProfile(id)` calls `switch_profile` with correct id | `expect(invoke).toHaveBeenCalledWith('switch_profile', { profileId: 42 })` |

---

### 4. `src/lib/stores/operations.svelte.test.ts`

**Requires both `invoke` and `listen` mocks.** Uses the listener-capture pattern described above.

Reset: `vi.resetModules()` + dynamic import in `beforeEach` + `await tick` to let IIFE register all listeners.

#### State transitions

| Test | Simulated event | Expected outcome |
|------|----------------|-----------------|
| initial state empty | — | `list.length === 0`, `hasAny === false` |
| adds pending op | `queue:updated` with `pending: [{id, label}]`, `active: null` | op in list with `status: 'pending'` |
| ignores duplicate pending | same `queue:updated` twice | still one op |
| removes cancelled pending | `queue:updated` with empty pending (op was cancelled) | op removed from list |
| transitions pending → running | `queue:updated` with `active: {id}` matching an existing pending | `status` becomes `'running'` |
| adds running op not seen before | `queue:updated` with `active` never seen as pending | op added with `status: 'running'` |
| `op:complete` marks done | fire `op:complete {id, message}` after running | `status: 'done'`, `result` set |
| `op:complete` clears pendingFiles | pending files populated, then complete | `pendingFiles: []` |
| `op:complete` flips active file to done | file log has active entry | becomes `status: 'done'` |
| `op:failed` marks error | fire `op:failed {id, error}` | `status: 'error'`, `result` set |
| `op:failed` clears pendingFiles | same as complete |

#### Progress / file log (`applyProgress`)

| Test | Details |
|------|---------|
| `backup:progress` updates `progress` | `current`, `total`, `detail` (basename) |
| progress uses basename of full path | `current_file: '/home/user/docs/file.txt'` → `detail: 'file.txt'` |
| first progress event adds active entry | `files` has one entry with `status: 'active'` |
| second progress flips previous active → done | two progress events → one done, one active |
| `op:pending_files` sets `pendingFiles` | array of names stored on op |
| progress removes current file from `pendingFiles` | fire `op:pending_files`, then progress → first name removed |
| file log capped at MAX_FILE_LOG (500) | fire 501 progress events → `files.length <= 501` (500 done + 1 active) |
| restore/scramble/verify/cleanup:progress all call same logic | each event type updates progress correctly |

#### Actions

| Test | Details |
|------|---------|
| `cancel()` on running op sets `cancelling: true` | check state before invoke |
| `cancel()` calls `invoke('cancel_operation', { opId })` | verify invoke arg |
| `cancel()` on pending op does not set `cancelling` | pending ops don't get the flag |
| `dismiss()` removes op from list | op gone after call |

---

### 5. Component tests (later phase)

These require `@testing-library/svelte` rendering and are heavier. Suggested targets in priority order:

#### `ConfirmModal.svelte` — good first component test

Pure props-in/events-out, no Tauri dependency, no stores.

- Renders `title` and `message`
- `confirmLabel` prop shows correct button text (defaults to "Confirm")
- `danger` prop applies danger styling to confirm button
- Clicking confirm fires `onconfirm` callback
- Clicking cancel fires `oncancel` callback
- Clicking overlay backdrop fires `oncancel`
- Escape key fires `oncancel`

#### `FileTable.svelte` — utility function coverage

The `formatSize(bytes)` function inside the component is a good candidate. Either:
- Extract it to `src/lib/utils/format.ts` and test in isolation (preferred)
- Or test via rendered component output

Expected cases: `0 B`, `1023 B`, `1 KB`, `1 MB`, `1 GB`, `1.5 KB`, large values.

#### `StatusFooter.svelte` — derived state logic

- Renders nothing when `operationsStore.list` is empty
- Shows active op label in header strip
- Shows pending count badge
- Shows finished count badge
- Expand toggle shows/hides op list
- File toggle shows/hides processed file list
- Remaining toggle shows/hides pending file list
- Progress bar width calculation (`current/total * 100`)
- File list capped at 100 + shows `+N more`

Needs `operationsStore` state seeded via the listener pattern or by directly manipulating module state.

---

## File layout

```
src/
├── test/
│   └── setup.ts                          # jest-dom import + global Tauri mocks
├── lib/
│   └── stores/
│       ├── selection.svelte.test.ts
│       ├── toast.svelte.test.ts
│       ├── profile.svelte.test.ts
│       └── operations.svelte.test.ts
└── lib/
    └── components/
        ├── ConfirmModal.test.ts
        ├── FileTable.test.ts
        └── StatusFooter.test.ts
```

## package.json additions

```json
{
  "scripts": {
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "devDependencies": {
    "vitest": "^3.x",
    "@testing-library/svelte": "^5.x",
    "@testing-library/jest-dom": "^6.x",
    "happy-dom": "^16.x"
  }
}
```

## Priority order

1. **selectionStore** — highest bang for buck, zero setup friction
2. **toastStore** — same, good timer coverage
3. **operationsStore** — highest value, covers the core state machine
4. **profileStore** — straightforward with invoke mock
5. **ConfirmModal** — first component test, establishes the rendering pattern
6. **FileTable `formatSize`** — extract utility, add pure function tests
7. **StatusFooter** — complex, do last when component test patterns are established
