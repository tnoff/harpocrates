# Development Guide

Setup, build, test, and dev-task patterns for working on Harpocrates. For
user-facing docs see [README.md](README.md). For non-obvious internals and
critical "do not break" rules see [AGENTS.md](AGENTS.md).

---

## Prerequisites

**Rust** — install via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
```

**Node.js 22 LTS** — via [nvm](https://github.com/nvm-sh/nvm): `nvm install`
(reads `.nvmrc`).

**Linux system deps:**

```bash
sudo apt install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev libdbus-1-dev libssl-dev pkg-config build-essential
```

Fedora/Arch equivalents: [Tauri prerequisites](https://tauri.app/start/prerequisites/).

---

## Running locally

```bash
git clone <repo-url>
cd harpocrates
npm install
npm run tauri dev     # starts Vite dev server + Tauri shell with hot-reload
```

Frontend only (no Tauri shell — most features won't work):

```bash
npm run dev           # http://localhost:5173
```

---

## Tests & linting

```bash
# Rust unit + integration tests
cargo test --manifest-path src-tauri/Cargo.toml --lib

# Rust linting (deny warnings)
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings

# Frontend unit tests (vitest + Testing Library)
npm test

# Frontend type-check
npm run check
```

Rust unit tests live in `#[cfg(test)] mod tests { ... }` blocks at the
bottom of each file; integration tests (in-memory SQLite) live in
`src-tauri/src/integration_tests.rs`. Use `tempfile::tempdir()` for
filesystem tests.

Frontend tests live alongside their subjects (e.g.
`ProfileForm.test.ts`, `routes/share/page.test.ts`, `stores/*.test.ts`).
Mock Tauri's `invoke` via `vi.mock('@tauri-apps/api/core')` — the global
mock is set up in `src/test/setup.ts`. Two test-time gotchas (timer
ordering, `$app/navigation` aliasing) are in
[AGENTS.md](AGENTS.md#non-obvious-gotchas).

---

## Building

```bash
npm run tauri build
# output: src-tauri/target/release/bundle/{deb,appimage,dmg,msi}/
```

---

## Project structure

```
src/                                  # SvelteKit frontend
├── routes/
│   ├── +layout.svelte                # Sidebar nav, profile switcher, StatusFooter
│   ├── +layout.ts                    # SSR disabled (adapter-static)
│   ├── +page.svelte                  # Root redirect → /setup or /files
│   ├── files/+page.svelte            # File browser, upload, restore, verify
│   ├── settings/+page.svelte         # Profiles, throttle, DB export/import
│   ├── share/+page.svelte            # Create/receive share manifests
│   ├── scramble/+page.svelte         # Re-key chunks; invalidates share manifests
│   ├── cleanup/+page.svelte          # Orphan detection and removal
│   └── setup/+page.svelte            # First-run wizard
└── lib/
    ├── components/                   # ProfileForm, FileTable, *Modal, StatusFooter, …
    └── stores/                       # All `.svelte.ts` with module-level $state

src-tauri/src/
├── lib.rs                            # App entry, managed state, invoke_handler
├── main.rs                           # Binary entry
├── commands.rs                       # Every #[tauri::command] fn
├── backup.rs                         # Chunk pipeline; make_chunk_s3_key()
├── queue.rs                          # Serial OperationQueue
├── s3.rs                             # S3Client
├── crypto.rs                         # XChaCha20-Poly1305, HMAC-SHA256
├── db.rs                             # SQLite schema + CRUD
├── profiles.rs                       # Profile CRUD (uses credentials.rs)
├── credentials.rs                    # Keyring wrapper
├── throttle.rs                       # Upload/download byte-rate limiter
├── config.rs                         # ~/.harpocrates/config.json
├── error.rs                          # AppError (thiserror), serializable
└── integration_tests.rs

scripts/set-version.mjs               # Stamps package.json version → Cargo.toml

.github/workflows/
├── ci.yml                            # Clippy + build on push/PR
└── release.yml                       # Tag + cross-platform build on push to main
```

---

## Svelte 5 runes

All state uses the runes API — **no Svelte 4 stores** (`writable`,
`readable`, etc.).

```typescript
let count = $state(0);                  // reactive state
let doubled = $derived(count * 2);      // derived (replaces $: )
$effect(() => { doSomething(count); }); // side effect
let { label, onclose } = $props();      // component props
```

Module-level `$state` (for shared stores) must live in `.svelte.ts` files:

```typescript
// src/lib/stores/my-store.svelte.ts
let items = $state<string[]>([]);
export const myStore = {
  get list() { return items; },
  add(item: string) { items = [...items, item]; },
};
```

There's an `untrack` snapshotting gotcha when capturing a prop at mount —
see [AGENTS.md](AGENTS.md#snapshot-a-prop-at-mount-with-untrack).

---

## Tauri IPC

### Commands

```typescript
import { invoke } from '@tauri-apps/api/core';
const result = await invoke<ReturnType>('command_name', { camelCaseParam: value });
```

Tauri auto-maps camelCase → snake_case across the boundary. All commands
return `Result<T, AppError>`; errors serialize to strings — use `String(e)`
in the catch block.

### Events (progress)

```typescript
import { listen } from '@tauri-apps/api/event';
const unlisten = await listen<Payload>('event:name', (event) => { ... });
// Always call unlisten() in finally {}
```

### App version

```typescript
import { getVersion } from '@tauri-apps/api/app';
const version = await getVersion(); // reads from Cargo.toml at build time
```

---

## Adding a Tauri command

1. Implement it in `src-tauri/src/commands.rs`:

   ```rust
   #[tauri::command]
   pub async fn my_command(
       app: tauri::AppHandle,    // for emit()
       db: State<'_, DbState>,
       some_param: String,
   ) -> Result<MyReturnType, AppError> {
       Ok(result)
   }
   ```

2. Register it in `src-tauri/src/lib.rs` inside `invoke_handler`:

   ```rust
   commands::my_command,
   ```

3. From the frontend:

   ```typescript
   const out = await invoke<MyReturnType>('my_command', { someParam: 'x' });
   ```

### Emitting progress

```rust
use tauri::Emitter;

#[derive(serde::Serialize, Clone)]
struct MyProgress { processed: usize, total: usize }

app.emit("my:progress", MyProgress { processed: i, total: n }).ok();
```

Event names follow the `noun:verb` convention.

### Error handling

Use `AppError` from `error.rs`. The `#[from]` derives handle `?`:

```rust
// Variants: Database, Io, Config, Serialization, Crypto, S3, Credential,
// Lock, NotFound, InvalidData
return Err(AppError::Config("something went wrong".into()));
```

Never `unwrap()` in a command handler — return an `AppError`.

---

## Operation queue

`src-tauri/src/queue.rs` runs a single worker that processes long-running
S3-touching ops (backup, restore, scramble, verify, cleanup) one at a
time. This keeps S3 access single-threaded and gives a unified
progress/cancel model.

```
Frontend                       Rust
--------                       ----
invoke("backup_directory")
  → queue.enqueue(...)     →   OperationQueue (FIFO channel)
  ← op_id (string)             worker task runs one op at a time
                                emits: queue:updated, backup:progress,
                                       op:complete, op:failed
invoke("cancel_operation",
       { opId })
  → queue.cancel(op_id)    →   sets AtomicBool; current file finishes,
                                then op stops
```

`queue.enqueue` returns immediately; the frontend never awaits the work.
Cancellation is cooperative — see
[AGENTS.md](AGENTS.md#cancellation-is-cooperative).

## Operations store pattern

Frontend mirrors the queue with `operationsStore`. **Do not use inline
progress UI in modals** — close the modal immediately, route progress
through the store, let `StatusFooter` render it.

```typescript
import { operationsStore } from '$lib/stores/operations.svelte';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';

async function startBackup() {
  // 1. Register the operation
  const id = operationsStore.add(`Backing up ${dirname}`, {
    oncancel: () => invoke('cancel_backup'),
  });

  // 2. Set up the progress listener
  const unlisten = await listen<BackupProgress>('backup:progress', (event) => {
    const p = event.payload;
    operationsStore.updateProgress(id, {
      current: p.processed,
      total: p.total,
      detail: p.current_file.split('/').at(-1),
    });
  });

  // 3. Close the modal immediately (user is unblocked)
  onclose();

  // 4. Invoke and update store on completion
  try {
    const summary = await invoke<BackupSummary>('backup_directory', { ... });
    operationsStore.complete(id, `${summary.uploaded} uploaded`);
  } catch (e) {
    operationsStore.fail(id, String(e));
  } finally {
    unlisten();
  }
}
```

Completed ops auto-dismiss after 5 seconds; errors stay until dismissed.

---

## Database schema

SQLite at `~/.harpocrates/harpocrates.db`. Schema is managed in
`src-tauri/src/db.rs`. Current version: **5** (constant `SCHEMA_VERSION`).
On startup, `init_database` checks `pragma user_version` and, if older,
drops and recreates all tables.

| Table | Key columns |
|-------|-------------|
| `profile` | `id, name, mode, s3_endpoint, s3_region, s3_bucket, extra_env, relative_path, temp_directory, s3_key_prefix, chunk_size_bytes, is_active` |
| `chunk` | `id, profile_id, chunk_hash, s3_key, encrypted_size` |
| `file_entry` | `id, profile_id, original_md5, total_size, chunk_count` |
| `file_chunk` | `id, file_entry_id, chunk_index, chunk_id` |
| `local_file` | `id, file_entry_id, local_path, cached_mtime, cached_size` |
| `share_manifest` | `id, profile_id, manifest_uuid, label, file_count, is_valid` |
| `share_manifest_entry` | `id, share_manifest_id, file_entry_id, filename` |

Key semantics:

- **`chunk.chunk_hash`** — `HMAC-SHA256(encryption_key, plaintext_chunk)`;
  both dedup key and resume marker.
- **`file_entry.original_md5`** — MD5 of whole plaintext file, computed
  as a rolling hash during chunking. Matching MD5 → outcome is `Deduped`;
  only `local_file` is upserted.
- **`profile.relative_path`** — local filesystem prefix stripped from
  paths before storing in `local_file.local_path`. Makes stored paths
  portable across machines. Has no effect on S3 keys.
- **`profile.s3_key_prefix`** — prepended to every S3 chunk/manifest key
  for the profile (enables per-prefix IAM). Validated by
  `profiles::validate_s3_key_prefix`: strips surrounding slashes /
  whitespace, rejects `//` and control chars, max 200 chars.

### Keyring

S3 credentials and encryption keys live in the OS keychain via
`credentials.rs`. Service name format: `harpocrates:{profile_name}:{key_type}`
where `key_type ∈ {s3-access-key, s3-secret-key, encryption-key}`.

### Chunk encryption

Each chunk is encrypted as `[12-byte random nonce][ciphertext + 16-byte
Poly1305 tag]` using XChaCha20-Poly1305. Chunks are small enough to
encrypt in memory — no temp files are written during backup.

---

## Versioning and release

`package.json` is the single source of truth for the version. To cut a
release:

```bash
# Bump version field in package.json, then:
git commit -am "chore: bump version to 0.2.0"
git tag v0.2.0 && git push origin main v0.2.0
```

CI's `release.yml` runs `node scripts/set-version.mjs` to stamp the
version from `package.json` into `Cargo.toml` before building, then
cross-builds for Linux/macOS/Windows and creates a draft GitHub Release.
The version shown in the sidebar comes from `getVersion()` reading
`Cargo.toml` at build time.

---

## CI

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| `ci.yml` | push/PR to `main` | Clippy (deny warnings), cargo build, npm build |
| `release.yml` | push to `main` (with version bump) | Tags from `package.json`, stamps version into `Cargo.toml`, builds for Linux/macOS/Windows, creates draft Release |

---

## IDE

VS Code extensions: Svelte for VS Code, Tauri, rust-analyzer,
Even Better TOML. For RustRover/CLion, open `src-tauri/` as the Cargo
project root.
