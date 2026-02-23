<script lang="ts">
  import "../app.css";
  import { profileStore, type Profile } from "$lib/stores/profile.svelte";
  import ToastContainer from "$lib/components/ToastContainer.svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import StatusFooter from "$lib/components/StatusFooter.svelte";
  import { getVersion } from "@tauri-apps/api/app";

  let { children } = $props();

  let showProfileMenu = $state(false);
  let appVersion = $state("");

  $effect(() => {
    getVersion().then((v) => (appVersion = v));
  });

  const navItems = $derived(
    profileStore.isReadOnly
      ? [{ label: "Share", href: "/share", icon: "↔" }]
      : [
          { label: "Files", href: "/files", icon: "📁" },
          { label: "Share", href: "/share", icon: "↔" },
          { label: "Scramble", href: "/scramble", icon: "🔀" },
          { label: "Cleanup", href: "/cleanup", icon: "🧹" },
        ]
  );

  const isSetup = $derived(page.url.pathname === "/setup");

  $effect(() => {
    profileStore.load().then(() => {
      if (!profileStore.active && profileStore.profiles.length === 0 && page.url.pathname !== "/setup") {
        goto("/setup");
      }
    });
  });

  function handleProfileSwitch(profile: Profile) {
    showProfileMenu = false;
    profileStore.switchProfile(profile.id);
  }
</script>

{#if isSetup}
  {@render children()}
{:else if profileStore.loading}
  <div class="layout-loading">
    <p>Loading...</p>
  </div>
{:else}
  <div class="layout-root">
    <!-- Sidebar -->
    <aside>
      <div class="sidebar-header">
        <h1>Harpocrates</h1>
        <p>Encrypted S3 Manager</p>
      </div>

      <!-- Profile Switcher -->
      <div class="profile-switcher">
        <button
          onclick={() => showProfileMenu = !showProfileMenu}
          class="profile-btn"
        >
          <div class="profile-btn-name">{profileStore.active?.name ?? "No profile"}</div>
          <div class="profile-btn-mode">{profileStore.active?.mode ?? ""}</div>
        </button>
        {#if showProfileMenu}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="profile-menu"
            onmouseleave={() => showProfileMenu = false}
          >
            {#each profileStore.profiles as profile}
              <button
                onclick={() => handleProfileSwitch(profile)}
                class="profile-menu-item"
                class:active={profile.id === profileStore.active?.id}
              >
                <div class="profile-menu-item-name">{profile.name}</div>
                <div class="profile-menu-item-mode">{profile.mode}</div>
              </button>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Navigation -->
      <nav>
        {#each navItems as item}
          <a
            href={item.href}
            class="nav-link"
            class:nav-link-active={page.url.pathname.startsWith(item.href)}
          >
            <span>{item.icon}</span>
            {item.label}
          </a>
        {/each}
      </nav>

      <!-- Settings link at bottom -->
      <div class="sidebar-footer">
        <a
          href="/settings"
          class="nav-link"
          class:nav-link-active={page.url.pathname === '/settings'}
        >
          <span>⚙</span>
          Settings
        </a>
        {#if appVersion}
          <p class="version-label">v{appVersion}</p>
        {/if}
      </div>
    </aside>

    <!-- Main content + footer -->
    <div class="content-area">
      <main>
        {@render children()}
      </main>
      <StatusFooter />
    </div>
  </div>
{/if}

<ToastContainer />

<style>
  .layout-root {
    display: flex;
    height: 100vh;
    background-color: #f8fafc;
  }

  .layout-loading {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    background-color: #f8fafc;
  }

  .layout-loading p {
    color: #64748b;
  }

  aside {
    width: 14rem;
    flex-shrink: 0;
    background-color: #f1f5f9;
    border-right: 1px solid #e2e8f0;
    display: flex;
    flex-direction: column;
  }

  .sidebar-header {
    padding: 1rem;
    border-bottom: 1px solid #e2e8f0;
  }

  .sidebar-header h1 {
    font-size: 1.125rem;
    font-weight: 700;
    letter-spacing: -0.025em;
    margin: 0 0 0.125rem;
  }

  .sidebar-header p {
    font-size: 0.75rem;
    color: #64748b;
    margin: 0;
  }

  .profile-switcher {
    position: relative;
    padding: 0.75rem;
    border-bottom: 1px solid #e2e8f0;
  }

  .profile-btn {
    width: 100%;
    text-align: left;
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    background: white;
    border: 1px solid #e2e8f0;
    font-size: 0.875rem;
    cursor: pointer;
    transition: border-color 0.15s;
  }

  .profile-btn:hover {
    border-color: #3b82f6;
  }

  .profile-btn-name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .profile-btn-mode {
    font-size: 0.75rem;
    color: #64748b;
  }

  .profile-menu {
    position: absolute;
    left: 0.75rem;
    right: 0.75rem;
    top: 100%;
    margin-top: 0.25rem;
    background: white;
    border: 1px solid #e2e8f0;
    border-radius: 0.5rem;
    box-shadow: 0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1);
    z-index: 50;
  }

  .profile-menu-item {
    width: 100%;
    text-align: left;
    padding: 0.5rem 0.75rem;
    font-size: 0.875rem;
    cursor: pointer;
    background: none;
    border: none;
    transition: background-color 0.15s;
  }

  .profile-menu-item:hover {
    background-color: #f1f5f9;
  }

  .profile-menu-item:first-child {
    border-radius: 0.5rem 0.5rem 0 0;
  }

  .profile-menu-item:last-child {
    border-radius: 0 0 0.5rem 0.5rem;
  }

  .profile-menu-item.active {
    background-color: #eff6ff;
  }

  .profile-menu-item-name {
    font-weight: 500;
  }

  .profile-menu-item-mode {
    font-size: 0.75rem;
    color: #64748b;
  }

  nav {
    flex: 1;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .nav-link {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.5rem;
    font-size: 0.875rem;
    text-decoration: none;
    color: #334155;
    transition: background-color 0.15s;
  }

  .nav-link:hover {
    background-color: #e2e8f0;
  }

  .nav-link-active {
    background-color: #3b82f6;
    color: white;
    font-weight: 500;
  }

  .nav-link-active:hover {
    background-color: #3b82f6;
  }

  .sidebar-footer {
    padding: 0.75rem;
    border-top: 1px solid #e2e8f0;
  }

  .version-label {
    font-size: 0.6875rem;
    color: #94a3b8;
    margin: 0.375rem 0 0;
    padding: 0 0.375rem;
  }

  .content-area {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  main {
    flex: 1;
    overflow: auto;
    padding: 1.5rem 1rem;
  }

  /* Dark mode */
  @media (prefers-color-scheme: dark) {
    .layout-root {
      background-color: #1e293b;
    }

    .layout-loading {
      background-color: #1e293b;
    }

    aside {
      background-color: #0f172a;
      border-right-color: #334155;
    }

    .sidebar-header {
      border-bottom-color: #334155;
    }

    .sidebar-header h1 {
      color: #f1f5f9;
    }

    .sidebar-header p {
      color: #94a3b8;
    }

    .profile-switcher {
      border-bottom-color: #334155;
    }

    .profile-btn {
      background-color: #1e293b;
      border-color: #475569;
      color: #f1f5f9;
    }

    .profile-btn:hover {
      border-color: #3b82f6;
    }

    .profile-btn-mode {
      color: #94a3b8;
    }

    .profile-menu {
      background-color: #1e293b;
      border-color: #475569;
    }

    .profile-menu-item {
      color: #f1f5f9;
    }

    .profile-menu-item:hover {
      background-color: #334155;
    }

    .profile-menu-item.active {
      background-color: #475569;
    }

    .profile-menu-item-mode {
      color: #94a3b8;
    }

    .nav-link {
      color: #cbd5e1;
    }

    .nav-link:hover {
      background-color: #334155;
    }

    .nav-link-active {
      background-color: #3b82f6;
      color: white;
    }

    .nav-link-active:hover {
      background-color: #3b82f6;
    }

    .sidebar-footer {
      border-top-color: #334155;
    }

    .version-label {
      color: #475569;
    }
  }
</style>
