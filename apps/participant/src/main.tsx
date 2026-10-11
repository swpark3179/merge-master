import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "@mm/shared/tokens.css";
import "@mm/shared/motion.css";
import "./app.css";
import "./edit.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
