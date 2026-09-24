// GhostDock's service worker: keeps the app itself available, never the data.
//
// The page is fetched network-first, so a new build is picked up as soon as
// the server can be reached, and the cached copy is used only when it cannot.
// Everything else the page loads is fingerprinted by the build and cached on
// first use. The API is never cached: its answers are private, go stale in
// seconds, and would show a board that looks live while it is not.
//
// Plain JavaScript because the browser runs this file directly; it has no
// build step and nothing else in the client is written in it.

const SHELL = 'ghostdock-shell';
const ASSETS = 'ghostdock-assets';

self.addEventListener('install', () => self.skipWaiting());

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
      if (previous && (await previous.text()) !== body) await caches.delete(ASSETS);
      await shell.put('/', fresh.clone());
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
