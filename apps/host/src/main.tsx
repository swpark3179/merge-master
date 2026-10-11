import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "@mm/shared/tokens.css";
import "./app.css";
import "./monitor.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
