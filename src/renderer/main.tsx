import React from "react";
import { createRoot } from "react-dom/client";
import "@fontsource/fusion-pixel-12px-proportional-sc";
import { App } from "./App";
import "./styles.css";

createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
