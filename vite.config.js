import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

/**
 * @tailwindcss/vite has `enforce:"pre"` and matches `&lang.css` in module IDs.
 * Svelte's virtual CSS modules use IDs like `file.svelte?svelte&type=style&lang.css`.
 * During Vite's pre-transform phase, these modules are intercepted by Tailwind before
 * the Svelte plugin has compiled the parent file, so Tailwind receives the raw .svelte
 * source (including <script>) and its CSS parser fails with "Invalid declaration: `invoke`".
 *
 * Fix: patch the plugin's transform filter to exclude any ID containing `?svelte`.
 * Tailwind still processes app.css and scans for class names via the Oxide scanner.
 */
function tailwindWithSvelteFix() {
  const plugins = tailwindcss();
  const arr = Array.isArray(plugins) ? plugins : [plugins];
  return arr.map((p) => {
    if (p?.transform?.filter?.id) {
      return {
        ...p,
        transform: {
          ...p.transform,
          filter: {
            ...p.transform.filter,
            id: {
              ...p.transform.filter.id,
              exclude: [...(p.transform.filter.id.exclude ?? []), /\?svelte/],
            },
          },
        },
      };
    }
    return p;
  });
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [sveltekit(), ...tailwindWithSvelteFix()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
