# Vault App — Architecture & Implementation Decisions

Tracks choices made during implementation that deviate from or clarify the original spec.

---

## Decision 1: Frontend Framework — Svelte
**Context**: Spec called for plain HTML/JS/CSS. The app has enough interactive complexity (multi-tab navigation, real-time progress bars, tables with multi-select, modals) that manual DOM management would become unwieldy.

**Choice**: Svelte (not SvelteKit — Tauri serves the frontend directly)

**Rationale**: Compiles to vanilla JS with tiny runtime, reactive state built-in, component model maps cleanly to the tab/view structure. Tauri v2's `create-tauri-app` has a Svelte template. No virtual DOM overhead.

**Alternatives considered**: Plain JS, Alpine.js/Preact, React, SolidJS

---

## Decision 2: Tauri v2
**Context**: Spec didn't specify a Tauri version.

**Choice**: Tauri v2

**Rationale**: Latest stable release, active development, improved plugin system and IPC model. Better to start on v2 than migrate later.

---

## Decision 3: S3 Crate — `aws-sdk-s3`
**Context**: Spec listed `aws-sdk-s3` or `rust-s3` as options.

**Choice**: `aws-sdk-s3`

**Rationale**: More full-featured, official AWS SDK, better maintained long-term. Handles multipart uploads natively. Works with S3-compatible endpoints via custom endpoint configuration.

---
