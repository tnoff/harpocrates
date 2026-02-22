# Development Guide

This guide covers setting up a local development environment, understanding the project structure, and common development workflows.

---

## Prerequisites

### Rust

Install via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
```

### Node.js

Node 20+ is required. Use [nvm](https://github.com/nvm-sh/nvm) or install directly:

```bash
nvm install 20
nvm use 20
```

### System Dependencies (Linux only)

```bash
# Ubuntu / Debian
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libdbus-1-dev \
  libssl-dev \
  pkg-config \
  build-essential
```

Fedora/RHEL and Arch equivalents are listed in the [Tauri prerequisites docs](https://tauri.app/start/prerequisites/).

### Tauri CLI

```bash
cargo install tauri-cli --version "^2"
# or via npm (slower but no cargo install needed):
npm install -g @tauri-apps/cli@^2
```

---

## Getting Started

```bash
git clone <repo-url>
cd vault-app
npm install
```

### Run in development mode

```bash
npm run tauri dev
```

This starts both the Vite dev server (hot-reload) and the Tauri shell. The Rust backend recompiles on file changes.

### Frontend only (no Tauri shell)

```bash
npm run dev
# http://localhost:5173
```

Note: Tauri IPC calls (`invoke`) will fail outside the Tauri shell, so most functionality won't work. Useful only for pure UI layout work.

---

## Project Structure

```
vault-app/
├── src/                        # SvelteKit frontend
│   ├── routes/                 # Pages (file-based routing)
│   │   ├── +layout.svelte      # Root layout: nav, profile switcher, ToastContainer
│   │   ├── +layout.ts          # SSR disabled (Tauri = static)
│   │   ├── +page.svelte        # Root redirect (→ /setup or /files)
│   │   ├── files/              # Main file browser
│   │   ├── settings/           # Profile management, bandwidth, DB tools
│   │   ├── share/              # Share manifest create/receive/manage
│   │   ├── scramble/           # Re-encrypt files
│   │   ├── cleanup/            # Orphan detection and removal
│   │   └── setup/              # First-run profile creation
│   └── lib/
│       ├── components/         # Reusable Svelte components
│       └── stores/             # Svelte 5 reactive stores
│
├── src-tauri/                  # Tauri + Rust backend
│   ├── src/
│   │   ├── lib.rs              # App entry point, managed state, command registry
│   │   ├── commands.rs         # All #[tauri::command] implementations
│   │   ├── backup.rs           # Directory backup logic, dedup, change detection
│   │   ├── s3.rs               # S3Client wrapper (upload, download, multipart, throttle)
│   │   ├── crypto.rs           # AES-256-GCM encrypt/decrypt, Argon2id key derivation
│   │   ├── db.rs               # SQLite schema, migrations, CRUD helpers
│   │   ├── profiles.rs         # Profile CRUD + keyring integration
│   │   ├── credentials.rs      # OS keychain read/write
│   │   ├── throttle.rs         # Global bandwidth throttle state
│   │   ├── config.rs           # App config (~/.vault/)
│   │   └── error.rs            # AppError type (thiserror)
│   ├── capabilities/
│   │   └── default.json        # Tauri v2 permission declarations
│   ├── Cargo.toml
│   └── tauri.conf.json         # App name, identifier, window config, bundle targets
│
├── .github/workflows/
│   ├── ci.yml                  # Lint + build check on push/PR
│   └── release.yml             # Cross-platform release builds on v* tags
│
├── README.md
├── DEVELOPMENT.md              # This file
├── SECURITY.md
├── package.json
└── tsconfig.json
```

---

## Frontend

### Tech Stack

| Tool | Version | Role |
|------|---------|------|
| Svelte | 5.x | UI framework (runes API) |
| SvelteKit | 2.x | Meta-framework, routing |
| TypeScript | 5.6 | Type safety |
| Tailwind CSS | 4.x | Utility-first styling |
| Vite | 6.x | Build tool / dev server |

### Svelte 5 Runes

This project uses the Svelte 5 runes API throughout. Key patterns:

```typescript
// Reactive state
let count = $state(0);

// Derived values
let doubled = $derived(count * 2);

// Side effects
$effect(() => { console.log(count); });

// Props
let { onclose }: { onclose: () => void } = $props();

// Reading reactive props without creating a dependency
import { untrack } from 'svelte';
const initial = untrack(() => props.value);
```

### Stores

| Store | File | Purpose |
|-------|------|---------|
| `profileStore` | `stores/profile.ts` | Active profile, profile list, `isReadOnly` |
| `selectionStore` | `stores/selection.ts` | Multi-select file IDs for batch operations |
| `toast` | `stores/toast.svelte.ts` | Toast notification queue |

Stores using `$state` at module level live in `.svelte.ts` files (required by the Svelte 5 compiler).

### Tauri IPC

All backend calls go through `invoke`:

```typescript
import { invoke } from '@tauri-apps/api/core';

const result = await invoke<ReturnType>('command_name', { param1: value1 });
```

Progress events use `listen`:

```typescript
import { listen } from '@tauri-apps/api/event';

const unlisten = await listen<ProgressPayload>('event:name', (event) => {
  progress = event.payload;
});
try {
  result = await invoke(...);
} finally {
  unlisten();
}
```

---

## Rust Backend

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `tauri` v2 | Desktop shell, IPC, event system |
| `rusqlite` | Bundled SQLite (no system dep) |
| `aws-sdk-s3` | S3 API client |
| `aes-gcm` | AES-256-GCM encryption |
| `argon2` | Key derivation |
| `keyring` | OS keychain (macOS/Win/Linux) |
| `tokio` | Async runtime |
| `thiserror` | Error type derivation |
| `regex` | Skip-pattern matching for directory backup |

### Adding a New Command

1. Write the function in `commands.rs`:

```rust
#[tauri::command]
pub async fn my_command(
    app: tauri::AppHandle,      // optional — needed for emit()
    db: State<'_, DbState>,
    some_param: String,
) -> Result<MyReturnType, AppError> {
    // ...
    Ok(result)
}
```

2. Register it in `lib.rs`:

```rust
.invoke_handler(tauri::generate_handler![
    // ...
    commands::my_command,
])
```

3. Call it from the frontend:

```typescript
const result = await invoke<MyReturnType>('my_command', { someParam: value });
```

Note: Tauri serializes command parameters using camelCase in TypeScript → snake_case in Rust automatically.

### Emitting Progress Events

```rust
use tauri::Emitter;

#[derive(serde::Serialize, Clone)]
struct MyProgressEvent { processed: usize, total: usize }

// Inside an async command with `app: tauri::AppHandle`:
let _ = app.emit("my:progress", MyProgressEvent { processed: i, total: n });
```

### Error Handling

All commands return `Result<T, AppError>`. The `AppError` type (in `error.rs`) implements `serde::Serialize` so errors surface as strings in the frontend `catch` block.

```rust
// error.rs variants:
AppError::Io(#[from] std::io::Error)
AppError::S3(String)
AppError::Crypto(String)
AppError::Db(#[from] rusqlite::Error)
AppError::Config(String)
AppError::General(String)
```

Frontend handling:

```typescript
try {
  result = await invoke('my_command', { ... });
} catch (e) {
  toast.error(String(e)); // AppError serializes to a human-readable string
}
```

### Database

Schema is defined and initialized in `db.rs`. The database is at `~/.vault/vault.db` (SQLite).

Tables:

| Table | Purpose |
|-------|---------|
| `profile` | S3 connection config per profile |
| `backup_entry` | One row per unique encrypted object in S3 |
| `local_file` | Maps a `backup_entry` to one or more local paths |
| `share_manifest` | A set of files shared via UUID token |
| `share_manifest_entry` | Files within a manifest |

The `backup_entry` + `local_file` split is what enables deduplication: multiple local paths can point at the same S3 object if their content (MD5) is identical.

---

## Running Tests

```bash
cd src-tauri
cargo test --lib
```

Tests live at the bottom of each Rust source file in `#[cfg(test)] mod tests { ... }` blocks. An in-memory SQLite database is used for DB tests.

Current coverage: 65 unit tests across `backup.rs`, `crypto.rs`, and `db.rs`.

---

## Linting and Type Checking

```bash
# Frontend (Svelte + TypeScript)
npm run check

# Rust
cd src-tauri
cargo clippy -- -D warnings
```

Both are enforced in CI on every push and pull request.

---

## Building for Release

### Local release build

```bash
npm run tauri build
```

Output is in `src-tauri/target/release/bundle/`:

| Platform | Output |
|----------|--------|
| Linux | `deb/` and `appimage/` |
| macOS | `dmg/` |
| Windows | `msi/` |

### CI release

Push a tag matching `v*`:

```bash
git tag v1.0.0
git push origin v1.0.0
```

The `release.yml` workflow builds for all platforms and creates a draft GitHub Release with all installers attached.

---

## IDE Setup

### VS Code (recommended)

Install these extensions:
- [Svelte for VS Code](https://marketplace.visualstudio.com/items?itemName=svelte.svelte-vscode)
- [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode)
- [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
- [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)

### RustRover / CLion

Open `src-tauri/` as the Cargo project root for the Rust side. Open the repo root separately for the frontend.
