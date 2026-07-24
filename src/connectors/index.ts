// Registry of connector setup tabs shown under Connectors.
//
// Add a connector: write a component taking `ConnectorTabProps` (own state, own
// Save button, persisting only its own config slice via `patchConfig`) and add
// an entry here. The Connectors page renders one sub-tab per entry, in order.

import GithubConnector from "./GithubConnector";
import type { ConnectorTab } from "./types";

export const CONNECTOR_TABS: ConnectorTab[] = [
  { id: "github", labelKey: "settings.github", Component: GithubConnector },
];

export type { ConnectorTab, ConnectorTabProps } from "./types";
