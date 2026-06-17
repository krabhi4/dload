// content/content.js — intercept plain left-clicks on magnet/.torrent anchors
// and forward them to the background. Modified clicks (ctrl/middle/etc.) are
// left alone so the browser's native "save link as" still works.

(function () {
  const SEL = [
    'a[href^="magnet:"]',
    'a[href$=".torrent" i]',
    'a[href*=".torrent?" i]',
    'a[href*=".torrent#" i]',
  ].join(", ");

  function attach(a) {
    if (!a || a.__dloadWired) return;
    a.__dloadWired = true;
    a.addEventListener(
      "click",
      (e) => {
        if (e.defaultPrevented) return;
        if (e.button !== 0) return; // only left click
        if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;

        const href = a.href;
        if (!href) return;

        e.preventDefault();
        e.stopImmediatePropagation();

        // Restore the default action (download / OS magnet handler) so we never
        // silently break a link the background declined to handle.
        const fallback = () => {
          try {
            window.location.href = href;
          } catch (err) {
            console.warn("[send-to-dload] fallback navigation failed:", err);
          }
        };

        let pending;
        try {
          pending = browser.runtime.sendMessage({
            type: "from-content-magnet",
            href,
            filename: a.textContent ? a.textContent.trim().slice(0, 200) : undefined,
          });
        } catch (err) {
          // Extension context invalidated (reloaded) — no background to reach.
          console.warn("[send-to-dload] sendMessage threw:", err);
          fallback();
          return;
        }

        Promise.resolve(pending)
          .then((resp) => {
            if (resp && resp.ok) return; // handled by dload
            // duplicate/queued: background already owns it — don't re-navigate.
            const reason = resp && resp.reason;
            if (reason === "duplicate" || reason === "queued") return;
            fallback();
          })
          .catch((err) => {
            console.warn("[send-to-dload] sendMessage failed:", err);
            fallback();
          });
      },
      true, // capture phase — beat page-level handlers
    );
  }

  function scan(root) {
    if (!root || !root.querySelectorAll) return;
    for (const a of root.querySelectorAll(SEL)) {
      attach(a);
    }
  }

  scan(document);

  // Catch anchors added later by SPAs.
  try {
    const obs = new MutationObserver((muts) => {
      for (const m of muts) {
        for (const node of m.addedNodes) {
          if (node.nodeType !== 1) continue;
          if (node.matches && node.matches(SEL)) attach(node);
          if (node.querySelectorAll) scan(node);
        }
      }
    });
    obs.observe(document.documentElement, { childList: true, subtree: true });
  } catch (err) {
    console.warn("[send-to-dload] MutationObserver unavailable:", err);
  }
})();