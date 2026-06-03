# AGENTS.md

Guidance for AI coding agents working on Harpocrates. For end-user docs see
[README.md](README.md); for setup, build, test, and dev conventions see
[DEVELOPMENT.md](DEVELOPMENT.md); for the threat model see
[SECURITY.md](SECURITY.md).

Tech stack: Tauri v2 + Rust backend + SvelteKit (Svelte 5 runes) frontend.

## Critical: do not break the Tailwind v4 + Svelte build patch

`vite.config.js` patches `@tailwindcss/vite` to exclude `?svelte` virtual
modules from its transform filter. **Do not remove or replace this patch.**

### Why the patch exists

`@tailwindcss/vite` uses `enforce: "pre"` and its transform filter matches
`&lang.css` in module IDs. Svelte's virtual CSS modules have IDs like
`file.svelte?svelte&type=style&lang.css` — they match the filter and get
intercepted by Tailwind during the pre-transform phase, *before* the Svelte
plugin has compiled the parent `.svelte`. At that point the module content
is raw `.svelte` source (with `<script>`), not extracted CSS. The CSS
parser then chokes on JS identifiers:

```
[plugin:@tailwindcss/vite:generate:serve] Invalid declaration: `invoke`
```

The `tailwindWithSvelteFix()` wrapper in `vite.config.js` is load-bearing.
Replacing it with a bare `tailwindcss()` call reintroduces the error.

### Related rule: no `@source` directives in `src/app.css`

`@source` causes Tailwind's CSS parser to read non-CSS files directly,
re-triggering the same crash. `src/app.css` must start with just:

```css
@import "tailwindcss";

@theme { ... }
```

Tailwind still discovers class names via the Oxide file scanner — a text
extractor that doesn't go through the CSS parser.

## Architectural conventions to follow

### Long-running ops go through the operation queue + operations store

All S3-touching ops (backup, restore, scramble, verify, cleanup) flow
through a single serial Rust queue in `src-tauri/src/queue.rs`. On the
frontend, register them with `operationsStore` and let `StatusFooter`
render progress — **do not embed inline progress UI in modals.** The
modal should close immediately on submit; the user is unblocked while the
op runs. Pattern is documented in [DEVELOPMENT.md](DEVELOPMENT.md#operations-store-pattern).

### Chunk identity is HMAC-SHA256, S3 keys are content-addressed

`chunk.chunk_hash = HMAC-SHA256(encryption_key, plaintext_chunk)` hex.
This is both the dedup key and the resume marker — a chunk already in the
DB for this profile is never re-uploaded.

S3 key format: `{prefix}/c/{chunk_hash}` (or `c/{chunk_hash}` without a
prefix). **Always use `backup::make_chunk_s3_key(prefix, hash)`** — do
not format manually. The function is unit-tested; ad-hoc formatting will
desync from cleanup/scramble/verify.

### v1 → v2 migration drops tables

`db::init_database` is at `SCHEMA_VERSION = 5`. On first open after
upgrading from v1, it **drops all tables** and recreates them — the
local file index is wiped. S3 objects are untouched. The v1 bare-UUID
objects then surface as orphans in the Cleanup tab.

This is by design (the schema change is too big to migrate in place), but
if a user reports "all my files disappeared after upgrade", that's why —
re-run Backup Directory and clean up old orphans.

## Non-obvious gotchas

### Snapshot a prop at mount with `untrack`

A prop bound via `$props()` is reactive — changes propagate to anywhere it
is read. When you only want the **mount-time value** (e.g. capturing an
initial selection), wrap in `untrack`:

```typescript
import { untrack } from 'svelte';
const initial = untrack(() => props.value);
```

Without `untrack`, `$state` derived from a prop re-evaluates whenever the
prop changes, which is rarely what you want for "initial value" semantics.

### `vi.useFakeTimers()` must come **after** the first `waitFor`

Fake timers break `waitFor`'s internal `setInterval` polling if enabled
before the first render settles. Order it as:

```typescript
await waitFor(() => expect(something).toBeInTheDocument());
vi.useFakeTimers();
```

### `$app/navigation` needs an explicit Vitest alias

`$app/navigation` is a SvelteKit virtual module that Vitest can't resolve.
Tests that render pages importing it must `vi.mock('$app/navigation', …)`.
The alias to `src/mocks/app-navigation.ts` is configured in
`vitest.config.ts`. Use `vi.hoisted(() => vi.fn())` for any mock values
referenced inside `vi.mock()` factory functions.

### Tauri auto-converts `invoke` param case

Tauri maps **camelCase** TypeScript params → **snake_case** Rust params
automatically. So `invoke('backup_directory', { dirPath: '/foo' })`
calls a Rust fn `pub async fn backup_directory(dir_path: String)`. The
case mismatch is *required*, not a bug — never write `dir_path` from the
frontend.

### Cancellation is cooperative

`queue.cancel(op_id)` sets an `AtomicBool`. The active op finishes the
current file before exiting; pending ops are removed from the queue
immediately. Don't expect mid-file cancellation — that would corrupt
partial uploads.

## File map

| Topic | Where |
|-------|-------|
| Tauri command registry | `src-tauri/src/lib.rs` `invoke_handler` |
| `#[tauri::command]` fns | `src-tauri/src/commands.rs` |
| Chunk pipeline (scan, dedup, encrypt, upload) | `src-tauri/src/backup.rs` |
| Serial op queue | `src-tauri/src/queue.rs` |
| S3 client + throttling | `src-tauri/src/s3.rs`, `throttle.rs` |
| XChaCha20-Poly1305 + HMAC | `src-tauri/src/crypto.rs` |
| SQLite schema + CRUD | `src-tauri/src/db.rs` |
| Keyring wrapper | `src-tauri/src/credentials.rs` |
| `AppError` enum | `src-tauri/src/error.rs` |
| Svelte stores (`.svelte.ts`) | `src/lib/stores/` |
| Operations store (footer-driven UI) | `src/lib/stores/operations.svelte.ts` |
| Page routes | `src/routes/<name>/+page.svelte` |
| Version bump script | `scripts/set-version.mjs` |

## Conventions

- All Svelte stores are `.svelte.ts` files with module-level `$state`. No
  Svelte 4 stores (`writable`, `readable`, etc.).
- Event names use `noun:verb`: `backup:progress`, `restore:progress`,
  `scramble:progress`. Match this when adding new ones.
- Modal overlays use `position: fixed; inset: 0` inside `<style>` blocks
  (not Tailwind utility classes), so they don't depend on Tailwind
  scanning class strings.
- Rust errors via `AppError` — never `unwrap()` in command handlers.
- Avoid adding npm dependencies without a strong reason; the frontend is
  intentionally lightweight.
