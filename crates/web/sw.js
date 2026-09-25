// GhostDock's service worker: keeps the app itself available, never the data.
//
// The page is fetched network-first, so a new build is picked up as soon as
// the server can be reached, and the cached copy is used only when it cannot
// or answers with a server error.
// Everything else the page loads is fingerprinted by the build and cached on
// first use. The API is never cached: its answers are private, go stale in
// seconds, and would show a board that looks live while it is not.
//
// Plain JavaScript because the browser runs this file directly; it has no
// build step and nothing else in the client is written in it.

const SHELL = 'ghostdock-shell';
const ASSETS = 'ghostdock-assets';
// The icons. The sign-in screen draws none, so the page does not load them
// until after it; kept ahead of time here so they are there offline even
// if no screen with an icon was opened online since the last build.
const SPRITE = '/icons/lucide.svg';

async function precache() {
  try {
    await (await caches.open(ASSETS)).add(SPRITE);
  } catch {
    // Offline now; it is cached on first use instead.
  }
}

self.addEventListener('install', event => {
  event.waitUntil(precache());
  self.skipWaiting();
});

self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    for (const name of await caches.keys()) {
      if (name !== SHELL && name !== ASSETS) await caches.delete(name);
    }
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', event => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== 'GET' || url.origin !== self.location.origin) return;
  if (url.pathname.startsWith('/api/')) return;

  if (request.mode === 'navigate') {
    event.respondWith(page(request));
  } else {
    event.respondWith(asset(request));
  }
});

async function page(request) {
  const shell = await caches.open(SHELL);
  try {
    const fresh = await fetch(request);
    if (fresh.ok) {
      const previous = await shell.match('/');
      const body = await fresh.clone().text();
      // A different page means a different build, whose assets have
      // different names; the old ones will never be asked for again.
      if (previous && (await previous.text()) !== body) {
        await caches.delete(ASSETS);
        await precache();
      }
      await shell.put('/', fresh.clone());
    }
    // A server that answers but is failing (a proxy in front of a
    // restarting GhostDock, say) is as good as offline: the app it served
    // last time can at least say what is wrong.
    if (fresh.status >= 500) {
      const cached = await shell.match('/');
      if (cached) return cached;
    }
    return fresh;
  } catch (offline) {
    const cached = await shell.match('/');
    if (cached) return cached;
    throw offline;
  }
}

async function asset(request) {
  const assets = await caches.open(ASSETS);
  const cached = await assets.match(request);
  if (cached) return cached;
  const fresh = await fetch(request);
  if (fresh.ok) await assets.put(request, fresh.clone());
  return fresh;
}
