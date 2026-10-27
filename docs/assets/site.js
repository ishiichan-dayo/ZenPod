(() => {
  const REPO = "ishiichan-dayo/ZenPod";
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  // Umami events (no personal data: only which control and which option)
  const track = (name, data) => { try { window.umami && window.umami.track(name, data); } catch (e) {} };

  // Light / dark: follow the OS until the visitor picks one, then remember it
  const toggle = $(".theme-toggle");
  if (toggle) toggle.addEventListener("click", () => {
    const root = document.documentElement;
    const dark = root.dataset.theme ? root.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
    root.dataset.theme = dark ? "light" : "dark";
    try { localStorage.setItem("theme", root.dataset.theme); } catch (e) {}
    track("theme-toggle", { to: root.dataset.theme });
  });

  // iPod OS | Rockbox switch above the app window
  const seg = $("[data-mode-seg]");
  const win = $(".window");
  if (seg && win) {
    const buttons = $$("button", seg);
    buttons.forEach((b, i) => b.addEventListener("click", () => {
      buttons.forEach((o) => o.setAttribute("aria-pressed", String(o === b)));
      seg.style.setProperty("--pos", i);
      win.dataset.mode = b.dataset.mode;
      $$("[data-note]").forEach((n) => (n.hidden = n.dataset.note !== b.dataset.mode));
      track("mode-switch", { mode: b.dataset.mode });
    }));
  }

  // Put the visitor's OS first
  const ua = navigator.userAgent;
  const os = /Windows/.test(ua) ? "windows" : /Mac OS X|Macintosh/.test(ua) && !/iPhone|iPad/.test(ua) ? "mac" : /Linux|X11/.test(ua) && !/Android/.test(ua) ? "linux" : null;
  const heroBtn = $("[data-hero-download]");
  if (os) {
    const card = $(`.dl[data-os="${os}"]`);
    if (card) { card.classList.add("current"); card.parentElement.prepend(card); }
    if (heroBtn && heroBtn.dataset[os]) heroBtn.querySelector("span").textContent = heroBtn.dataset[os];
  }
  if (heroBtn) heroBtn.dataset.umamiEventOs = os || "other";

  // Intro video: started and watched to the end
  const video = $(".video video");
  if (video) {
    video.addEventListener("play", () => track("video-play"), { once: true });
    video.addEventListener("ended", () => track("video-complete"), { once: true });
  }

  // FAQ: which questions get opened
  $$("details[data-faq]").forEach((d) => d.addEventListener("toggle", () => {
    if (d.open) track("faq-open", { q: d.dataset.faq });
  }));

  // How far people read: each section once, when its top passes 60% of the screen
  if ("IntersectionObserver" in window) {
    const seen = new Set();
    const io = new IntersectionObserver((entries) => entries.forEach((e) => {
      const id = e.target.id;
      if (!e.isIntersecting || seen.has(id)) return;
      seen.add(id);
      io.unobserve(e.target);
      track("section-view", { section: id });
    }), { rootMargin: "0px 0px -40% 0px" });
    $$("main section[id]").forEach((el) => io.observe(el));
  }

  // Direct links to the files of the latest release (falls back to the release page)
  const SUFFIX = { windows: "_x64-setup.exe", mac: "_universal.dmg", appimage: "_amd64.AppImage", deb: "_amd64.deb" };
  fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } })
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((rel) => {
      for (const [key, suffix] of Object.entries(SUFFIX)) {
        const asset = rel.assets.find((a) => a.name.endsWith(suffix));
        if (!asset) continue;
        $$(`[data-asset="${key}"]`).forEach((a) => {
          a.href = asset.browser_download_url;
          const size = $(".size", a);
          if (size) size.textContent = `${Math.round(asset.size / 1e6)} MB`;
        });
      }
      if (os && heroBtn) {
        const own = $(`[data-asset="${os === "linux" ? "appimage" : os}"]`);
        if (own) heroBtn.href = own.href;
      }
      const v = $("[data-version]");
      if (v) {
        const date = new Date(rel.published_at).toLocaleDateString(document.documentElement.lang, { year: "numeric", month: "long", day: "numeric" });
        v.textContent = v.dataset.template.replace("{version}", rel.tag_name.replace(/^v/, "")).replace("{date}", date);
        v.hidden = false;
      }
    })
    .catch(() => {});
})();
