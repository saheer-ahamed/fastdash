import type { ComponentType } from "react";

// Props every connector setup tab receives. A tab owns its own state and its
// own Save button, and persists only its own slice of the config.
export interface ConnectorTabProps {
  /// Ask the app to re-fetch this connector's dashboard after a successful save.
  onRefresh: () => void;
  /// Show a transient success banner on the Connectors page.
  flash: (msg: string) => void;
  /// Show a transient error banner on the Connectors page.
  error: (e: unknown) => void;
}

export interface ConnectorTab {
  /// Connector id, matching the backend registry (used for `onRefresh`).
  id: string;
  /// i18n key for the sub-tab label.
  labelKey: string;
  Component: ComponentType<ConnectorTabProps>;
}
