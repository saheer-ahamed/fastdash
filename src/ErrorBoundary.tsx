import { Component, type ErrorInfo, type ReactNode } from "react";
import { t } from "./i18n";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
  info: ErrorInfo | null;
}

// Catches render/lifecycle errors anywhere below it so a single bad panel can't
// blank the whole window. Renders a flat, monospace crash screen that matches
// the terminal aesthetic (see DESIGN.md): monochrome, no radius, `//`/`##`
// prefixes, and the `--err` accent used by status banners.
//
// A class component is required: componentDidCatch / getDerivedStateFromError
// have no hook equivalent.
export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, info: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
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
    const { error, info } = this.state;
    if (!error) return this.props.children;

    // Technical detail is kept out of the way: the screen leads with a plain,
    // reassuring message, and the error text + component stack live inside a
    // collapsed section for anyone filing a bug report.
    const technical = [error.stack ?? error.message ?? String(error), info?.componentStack]
      .map((s) => s?.trim())
      .filter(Boolean)
      .join("\n\n");

    return (
      <div className="error-screen" role="alert">
        <div className="error-box">
          <div className="brand">fastdash</div>
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

          {technical && (
            <details className="error-stack">
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
