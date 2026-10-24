import React from "react";
import { createRoot } from "react-dom/client";
import "@fontsource/fusion-pixel-12px-proportional-sc";
import { App } from "./App";
import { desktopApi } from "./desktopApi";
import "./styles.css";

function reportError(source: "error" | "unhandledrejection", message: string) {
  void desktopApi.reportFrontendError(source, message).catch((error) => {
    console.warn("记录前端错误失败", error);
  });
}

if (window.__TAURI_INTERNALS__) {
  window.addEventListener("error", (event) => {
    const error: unknown = event.error;
    reportError("error", error instanceof Error ? `${error.message}\n${error.stack ?? ""}` : `${event.message} ${event.filename}:${event.lineno}`);
  });
  window.addEventListener("unhandledrejection", (event) => {
    const reason: unknown = event.reason;
    reportError("unhandledrejection", reason instanceof Error ? `${reason.message}\n${reason.stack ?? ""}` : String(reason));
  });
}

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
