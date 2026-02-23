# Harpocrates — Agent Guide

This document is the primary reference for AI agents (Claude, Cursor, Copilot, etc.) working on this codebase. Read it before making changes.

---

## What Is This App

**Harpocrates** is a cross-platform desktop app (Tauri v2) that encrypts files client-side with AES-256-GCM and backs them up to any S3-compatible bucket. The user's encryption key is generated at first use and never stored by the app — only the user holds it.

Tech stack: **Tauri v2 + Rust backend + SvelteKit (Svelte 5 runes) frontend**.

---

## Project Structure

```
harpocrates/
├── src/                        # SvelteKit frontend
│   ├── routes/
│   │   ├── +layout.svelte      # Root layout: sidebar nav, profile switcher, StatusFooter
│   │   ├── +layout.ts          # SSR disabled (adapter-static)
│   │   ├── +page.svelte        # Root redirect → /setup or /files
│   │   ├── files/+page.svelte  # File browser, upload, restore, verify
│   │   ├── settings/+page.svelte # Profiles, bandwidth throttle, DB export/import
│   │   ├── share/+page.svelte  # Create/receive/manage share manifests
│   │   ├── scramble/+page.svelte # Re-encrypt files with new UUIDs
│   │   ├── cleanup/+page.svelte  # Orphan detection and removal
│   │   └── setup/+page.svelte  # First-run profile creation wizard
│   └── lib/
│       ├── components/
│       │   ├── ProfileForm.svelte          # S3 connection form (shared by setup + settings)
│       │   ├── FileTable.svelte            # File list with checkbox multi-select
│       │   ├── BackupDirectoryModal.svelte # Directory backup config; closes immediately, tracks via operations store
│       │   ├── RestoreModal.svelte         # Restore destination config; same pattern
│       │   ├── VerifyIntegrityModal.svelte # Shows per-file verification results
│       │   ├── ConfirmModal.svelte         # Generic yes/no modal
│       │   ├── StatusFooter.svelte         # Persistent bottom bar for in-progress ops
│       │   └── ToastContainer.svelte       # Fixed-position toast notifications
│       └── stores/
│           ├── profile.svelte.ts    # Active profile + list + isReadOnly
│           ├── selection.svelte.ts  # Selected file IDs (Set<number>)
│           ├── toast.svelte.ts      # Toast queue (success/error)
│           └── operations.svelte.ts # Global operation tracking for StatusFooter
│
├── src-tauri/src/
│   ├── lib.rs              # App entry, managed state, invoke_handler registry
│   ├── main.rs             # Binary entry point (calls harpocrates_lib::run())
│   ├── commands.rs         # Every #[tauri::command] fn
│   ├── backup.rs           # Directory backup: change detection, dedup, skip patterns
│   ├── s3.rs               # S3Client: upload, download, list, multipart, throttle
│   ├── crypto.rs           # AES-256-GCM + Argon2id; temp file prefix: "harpocrates-tmp-"
│   ├── db.rs               # SQLite schema + CRUD
│   ├── profiles.rs         # Profile CRUD, integrates credentials.rs
│   ├── credentials.rs      # Keyring wrapper; service prefix: "harpocrates"
│   ├── throttle.rs         # Global upload/download byte-rate limiter
│   ├── config.rs           # Config file at ~/.harpocrates/config.json
│   ├── error.rs            # AppError (thiserror); serializable for Tauri IPC
│   └── integration_tests.rs
│
├── scripts/
│   └── set-version.mjs     # Stamps VERSION → Cargo.toml + package.json
│
├── .github/workflows/
│   ├── ci.yml              # Clippy + build on push/PR
│   └── release.yml         # Cross-platform build on v* tag push
│
└── VERSION                 # e.g. "0.1.0" — single source of truth for releases
```

---

## ⚠ Critical: Tailwind v4 + SvelteKit — `vite.config.js` patch required

**This has caused repeated build-breaking errors. The fix lives in `vite.config.js`.**

### The conflict

`@tailwindcss/vite` uses `enforce: "pre"` and its transform filter matches `&lang.css` in module IDs. Svelte's virtual CSS modules have IDs like `file.svelte?svelte&type=style&lang.css` — matching that filter. During Vite's pre-transform phase, these modules are intercepted by Tailwind *before* the Svelte plugin has compiled the parent `.svelte` file. At that point the module content is the raw `.svelte` source (including `<script>`), not the extracted CSS. The CSS parser then chokes on JavaScript identifiers:

```
[plugin:@tailwindcss/vite:generate:serve] Invalid declaration: `invoke`
[plugin:@tailwindcss/vite:generate:serve] Invalid declaration: `selectionStore`
```

### The fix

`vite.config.js` patches the `@tailwindcss/vite` plugin to exclude any module ID containing `?svelte` from its transform filter. Tailwind still processes `app.css` normally and scans for class names via the Oxide file scanner (text extractor, not CSS parser). Only the incorrect interception of Svelte virtual CSS modules is blocked.

**Do not remove this patch.** The `tailwindWithSvelteFix()` wrapper in `vite.config.js` is load-bearing. Replacing it with a bare `tailwindcss()` call will reintroduce the error.

**`src/app.css` must NOT have `@source` directives** — these also cause Tailwind's CSS parser to read non-CSS files directly.

`src/app.css` should start with just:

```css
@import "tailwindcss";

@theme { ... }
```

### Styling Rules

1. **Every Svelte component has a `<style>` block** with semantic class names. Scoped component styles are always safe and don't depend on scanning.

2. **Use `class:` directives for dynamic states:**
   ```svelte
   <!-- CORRECT -->
   <button class="tab-btn" class:active={activeTab === "create"}>Create</button>

   <!-- AVOID — concatenated class strings may not be scanned -->
   <button class={"tab-btn" + (activeTab === "create" ? " active" : "")}>Create</button>
   ```

3. **Global reusable classes** are defined in `src/app.css` under `@layer components`. Current globals: `.btn-primary`, `.btn-secondary`, `.form-input`, `.form-label`, `.form-hint`, `.form-warning`. Use these freely in templates.

4. **Inline `style=""` is fine** for values that come from JavaScript (e.g. progress bar widths, dynamic colors).

5. **Modal overlays must use `position: fixed` in a `<style>` block**, not `class="fixed inset-0 ..."` — prefer scoped styles for layout-critical properties.

---

## Svelte 5 Runes

All state uses the Svelte 5 runes API — **no Svelte 4 stores (`writable`, `readable`, etc.)**.

```typescript
let count = $state(0);                    // reactive state
let doubled = $derived(count * 2);        // derived (replaces $: )
$effect(() => { doSomething(count); });   // side effect (replaces onMount + reactive statements)
let { label, onclose } = $props();        // component props

// Snapshot a prop at mount time (avoid re-running when prop changes):
import { untrack } from 'svelte';
const initialValue = untrack(() => props.value);
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

---

## Operations Store Pattern

Long-running tasks (backup, restore, scramble) follow this pattern — **do not use inline progress UI in modals**:

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

The `StatusFooter` component reads `operationsStore.list` and renders all ops. Completed ops auto-dismiss after 5 seconds; errors stay until dismissed.

---

## Tauri IPC

### Commands

```typescript
import { invoke } from '@tauri-apps/api/core';
const result = await invoke<ReturnType>('command_name', { camelCaseParam: value });
```

Tauri automatically maps **camelCase** TypeScript params → **snake_case** Rust params.

All commands return `Result<T, AppError>`. Errors serialize to human-readable strings — use `String(e)` in the catch block.

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

## Rust Backend

### Adding a Command

1. Add to `commands.rs`:
```rust
#[tauri::command]
pub async fn my_command(
    app: tauri::AppHandle,   // for emit()
    db: State<'_, DbState>,
    some_param: String,
) -> Result<MyReturnType, AppError> {
    Ok(result)
}
```

2. Register in `lib.rs` inside `invoke_handler`:
```rust
commands::my_command,
```

### Emitting Progress

```rust
use tauri::Emitter;
#[derive(serde::Serialize, Clone)]
struct MyProgress { processed: usize, total: usize }

app.emit("my:progress", MyProgress { processed: i, total: n }).ok();
```

### Error Handling

Use `AppError` variants from `error.rs`. The `#[from]` derives handle `?` conversions:

```rust
// Variants: Io, S3, Crypto, Db, Credential, Config, General
return Err(AppError::General("something went wrong".into()));
```

### Database

SQLite at `~/.harpocrates/harpocrates.db`. Schema managed in `db.rs`.

| Table | Key columns |
|-------|-------------|
| `profile` | id, name, mode, s3_endpoint, s3_bucket, relative_path, is_active |
| `backup_entry` | id, s3_key (UUID), original_md5, encrypted_md5, file_size |
| `local_file` | id, backup_entry_id, local_path, file_name |
| `share_manifest` | id, uuid, name, created_at |
| `share_manifest_entry` | manifest_id, backup_entry_id, file_name |

Deduplication works by matching `original_md5` — two local files with identical content share one `backup_entry` and one S3 object.

### Keyring

S3 credentials and encryption keys are stored in the OS keychain via `credentials.rs`. Service names follow the pattern `harpocrates:{profile_name}:{key_type}` where `key_type` is one of `s3-access-key`, `s3-secret-key`, `encryption-key`.

### Temp Files

Temporary files during encryption/decryption use the prefix `harpocrates-tmp-{uuid}`. They are cleaned up at startup and after each operation. If a leftover temp file is found, it is safe to delete.

---

## Local Paths

| Item | Path |
|------|------|
| Config dir | `~/.harpocrates/` |
| Database | `~/.harpocrates/harpocrates.db` |
| Config file | `~/.harpocrates/config.json` |
| Temp files | System temp dir, prefix `harpocrates-tmp-` |
| Keyring service | `harpocrates:{profile}:{key_type}` |

---

## Versioning

`VERSION` (repo root) is the single source of truth. To release:

```bash
echo "0.2.0" > VERSION
git commit -am "chore: bump version to 0.2.0"
git tag v0.2.0 && git push origin main v0.2.0
```

The release CI runs `node scripts/set-version.mjs` to stamp the version into `Cargo.toml` and `package.json` before building. The version appears in the app sidebar via `getVersion()`.

---

## Tests

```bash
cd src-tauri
cargo test --lib
```

Unit tests are in `#[cfg(test)] mod tests { ... }` blocks at the bottom of each Rust file. Use `tempfile::tempdir()` for filesystem tests. Integration tests (in-memory SQLite) live in `integration_tests.rs`.

Frontend type-checking:

```bash
npm run check
```

---

## CI

| Workflow | Trigger | What it does |
|----------|---------|-------------|
| `ci.yml` | push/PR to `main` | Clippy (deny warnings), cargo build, npm build |
| `release.yml` | push `v*` tag | Stamps version, builds for Linux/macOS/Windows, creates draft GitHub Release |

---

## Conventions

- **No `@source` directives in `app.css`** — causes CSS parser crashes. See critical section.
- **Do not replace `tailwindWithSvelteFix()` with bare `tailwindcss()`** in `vite.config.js` — see critical section.
- **No inline progress UI in modals** — use the operations store + StatusFooter.
- **Rust errors via `AppError`** — never `unwrap()` in command handlers.
- **camelCase params in `invoke()`** → auto-converted to `snake_case` in Rust.
- **Event names** use `noun:verb` format: `backup:progress`, `restore:progress`, `scramble:progress`.
- **All stores are `.svelte.ts`** files with module-level `$state`.
- **Modal overlays** use `position: fixed; inset: 0` in `<style>` blocks.
- Avoid adding new npm dependencies without a strong reason — the frontend is intentionally lightweight.
