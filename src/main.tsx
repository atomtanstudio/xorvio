import "@fontsource-variable/inter";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { api, initBackend } from "./lib/api";
import { isPopOutWindow } from "./lib/windows";
import { PopOutApp } from "./player/PopOut";
import "./styles/tokens.css";
import "./styles/app.css";
import "./styles/screens.css";

// Right-clicks open Xorvio's own menus, never the web view's.
window.addEventListener("contextmenu", (e) => {
  if (!(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) e.preventDefault();
});

initBackend().then(() => {
  const report = (message: string) => api.log("error", message).catch(() => {});
  window.addEventListener("error", (e) => report(`uncaught: ${e.message} at ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => report(`unhandled rejection: ${e.reason instanceof Error ? e.reason.message : String(e.reason)}`));

  createRoot(document.getElementById("root")!).render(
    <StrictMode>{isPopOutWindow ? <PopOutApp /> : <App />}</StrictMode>,
  );
});
