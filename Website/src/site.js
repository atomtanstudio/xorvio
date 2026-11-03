// Xorvio site: nav state, one-time reveals, the screen tour and the screenshot viewer.
document.documentElement.classList.add("js");

document.addEventListener("DOMContentLoaded", () => {
  const nav = document.querySelector("[data-nav]");
  const onScroll = () => nav && nav.classList.toggle("is-scrolled", window.scrollY > 12);
  onScroll();
  window.addEventListener("scroll", onScroll, { passive: true });

  // Each element fades in once as it enters the viewport; nothing moves after that.
  const reveals = document.querySelectorAll(".reveal");
  if ("IntersectionObserver" in window) {
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          entry.target.classList.add("is-in");
          observer.unobserve(entry.target);
        }
      }
    }, { rootMargin: "0px 0px -8% 0px", threshold: 0.08 });
    reveals.forEach((el) => observer.observe(el));
  } else {
    reveals.forEach((el) => el.classList.add("is-in"));
  }

  // The tour: a tab list that swaps screenshots.
  for (const tour of document.querySelectorAll("[data-tour]")) {
    const tabs = [...tour.querySelectorAll('[role="tab"]')];
    const select = (tab, focus) => {
      for (const t of tabs) {
        const on = t === tab;
        t.setAttribute("aria-selected", String(on));
        t.tabIndex = on ? 0 : -1;
        const panel = document.getElementById(t.getAttribute("aria-controls"));
        panel.hidden = !on;
        panel.classList.toggle("is-active", on);
        if (on) panel.querySelectorAll("img[loading=lazy]").forEach((img) => (img.loading = "eager"));
      }
      if (focus) tab.focus();
    };
    tabs.forEach((tab, i) => {
      tab.addEventListener("click", () => select(tab));
      tab.addEventListener("keydown", (e) => {
        const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
        if (step) {
          e.preventDefault();
          select(tabs[(i + step + tabs.length) % tabs.length], true);
        }
      });
    });
  }

  // The launch film: the page's own play button stands in for the browser's controls
  // until the film plays (Safari would otherwise draw its play glyph over the poster's
  // lockup); then the browser's controls take over for pause, scrubbing and full screen.
  for (const figure of document.querySelectorAll("[data-film]")) {
    const video = figure.querySelector("video");
    if (!video) continue;
    video.controls = false;
    const play = document.createElement("button");
    play.type = "button";
    play.className = "film__play";
    play.setAttribute("aria-label", "Play the film");
    play.innerHTML = '<span><svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M7 4.5v15l12.5-7.5z"/></svg></span>';
    const started = () => {
      play.remove();
      video.controls = true;
    };
    play.addEventListener("click", () => video.play().catch(started));
    video.addEventListener("play", started, { once: true });
    video.after(play);
  }

  // Screenshot viewer: click a screenshot to see it at full size.
  const zoomables = document.querySelectorAll("[data-zoom]");
  if (zoomables.length && "HTMLDialogElement" in window) {
    const dialog = document.createElement("dialog");
    dialog.className = "lightbox";
    dialog.innerHTML = '<button type="button" aria-label="Close">✕</button><img alt="">';
    document.body.append(dialog);
    const img = dialog.querySelector("img");
    let opener = null;
    let openedByPointer = false;
    dialog.addEventListener("click", () => dialog.close());
    // Focus goes back to the screenshot on close; only keep its ring for keyboard users.
    dialog.addEventListener("close", () => {
      if (openedByPointer && opener) opener.blur();
    });
    for (const el of zoomables) {
      el.addEventListener("click", (event) => {
        const source = el.querySelector("img");
        opener = el;
        openedByPointer = event.detail > 0;
        img.src = el.dataset.zoom;
        img.alt = source ? source.alt : "";
        dialog.showModal();
      });
    }
  }
});
