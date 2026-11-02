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
