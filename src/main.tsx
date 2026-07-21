import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ErrorBoundary from "./ErrorBoundary";
import { initTheme } from "./theme";
import { runUpdateCheck } from "./updater";
import "./styles.css";

initTheme();

// Fire-and-forget: never let the update check delay first paint.
void runUpdateCheck();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
