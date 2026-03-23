# Development Guide

---

## Prerequisites

**Rust** — install via [rustup](https://rustup.rs/):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
```

**Node.js 22 LTS** — via [nvm](https://github.com/nvm-sh/nvm): `nvm install` (reads `.nvmrc`).

**Linux system deps:**
```bash
sudo apt install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev libdbus-1-dev libssl-dev pkg-config build-essential
```
Fedora/Arch equivalents: [Tauri prerequisites](https://tauri.app/start/prerequisites/).

---

## Running Locally

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

## Tests & Linting

```bash
# Rust unit + integration tests
cargo test --manifest-path src-tauri/Cargo.toml --lib

# Rust linting
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings

# Frontend unit tests
npm test

# Frontend type-check
npm run check
```

---

## Building

```bash
npm run tauri build
# output: src-tauri/target/release/bundle/{deb,appimage,dmg,msi}/
```

**Release build via CI:** update the version in `package.json`, commit, push a `v*` tag. See `AGENTS.md` for the full release flow.

---

## IDE

**VS Code extensions:** Svelte for VS Code, Tauri, rust-analyzer, Even Better TOML.

**RustRover/CLion:** open `src-tauri/` as the Cargo project root.
