import { Component, type ErrorInfo, type ReactNode } from "react";
import { t } from "./i18n";
import { isDevMode, onDevModeChange } from "./devmode";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
  info: ErrorInfo | null;
  devMode: boolean;
}

// Catches render/lifecycle errors anywhere below it so a single bad panel can't
// blank the whole window. Renders a flat, monospace crash screen that matches
// the terminal aesthetic (see DESIGN.md): monochrome, no radius, `//`/`##`
// prefixes, and the `--err` accent used by status banners.
//
// A class component is required: componentDidCatch / getDerivedStateFromError
// have no hook equivalent.
export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, info: null, devMode: isDevMode() };
  private unsubscribe?: () => void;

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidMount(): void {
    // Re-render if dev mode is toggled while a crash screen is up, so the
    // technical detail expands (or collapses) to match.
    this.unsubscribe = onDevModeChange(() => this.setState({ devMode: isDevMode() }));
  }

  componentWillUnmount(): void {
    this.unsubscribe?.();
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Surface to the devtools console; the on-screen panel carries the same
    // detail for release builds where the console isn't open.
    console.error("Unhandled UI error:", error, info.componentStack);
    this.setState({ info });
  }

  private reset = (): void => {
    this.setState({ error: null, info: null });
  };

  private copyDetails = (): void => {
    const { error, info } = this.state;
    const details = [
      error?.stack ?? String(error),
      info?.componentStack ?? "",
    ]
      .filter(Boolean)
      .join("\n\n");
    void navigator.clipboard?.writeText(details).catch(() => {});
  };

  render(): ReactNode {
    const { error, info, devMode } = this.state;
    if (!error) return this.props.children;

    // Everyday users only ever see the plain, reassuring message. The technical
    // error text + component stack are developer-only: they appear (expanded)
    // solely in developer mode, and are hidden entirely otherwise.
    const technical = [error.stack ?? error.message ?? String(error), info?.componentStack]
      .map((s) => s?.trim())
      .filter(Boolean)
      .join("\n\n");

    return (
      <div className="error-screen" role="alert">
        <div className="error-box">
          <div className="error-brandline">
            <span className="brand">fastdash</span>
            {devMode && <span className="dev-badge">{t("error.devBadge")}</span>}
          </div>
          <h1 className="error-title">{t("error.title")}</h1>
          <p className="error-lede">{t("error.lede")}</p>
          <p className="error-hint muted">{t("error.hint")}</p>

          <div className="error-actions">
            <button className="save-btn" onClick={this.reset}>
              {t("error.retry")}
            </button>
            <button className="link-btn" onClick={() => window.location.reload()}>
              {t("error.reload")}
            </button>
          </div>

          {devMode && technical && (
            <details className="error-stack" open>
              <summary>{t("error.details")}</summary>
              <pre>{technical}</pre>
              <button className="link-btn error-copy" onClick={this.copyDetails}>
                {t("error.copy")}
              </button>
            </details>
          )}
        </div>
      </div>
    );
  }
}
