import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import BootGate from "./BootGate";
import ErrorBoundary from "./ErrorBoundary";
import { initTheme } from "./theme";
import "./styles.css";

initTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BootGate>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </BootGate>
  </React.StrictMode>,
);
