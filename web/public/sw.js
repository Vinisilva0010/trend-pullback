// Service worker: shows push notifications and opens the dashboard on tap.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let data = { title: "Trend Pullback", body: "", url: "/avisos", tag: "aviso" };
  try {
    data = { ...data, ...event.data.json() };
  } catch {
    // Keeps the default text.
  }
  event.waitUntil(
    self.registration
      .showNotification(data.title, {
        body: data.body,
        tag: data.tag,
        icon: "/icon-192.png",
        badge: "/icon-192.png",
        data: { url: data.url },
      })
      // Red number on the app icon: notifications still waiting to be seen.
      .then(() => self.registration.getNotifications())
      .then((list) => (self.navigator.setAppBadge ? self.navigator.setAppBadge(list.length) : undefined))
      .catch(() => undefined),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = new URL(event.notification.data?.url || "/avisos", self.location.origin);
  if (url.origin !== self.location.origin) return;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((list) => {
      for (const client of list) {
        if ("focus" in client) {
          client.navigate(url.href);
          return client.focus();
        }
      }
      return self.clients.openWindow(url.href);
    }),
  );
});
