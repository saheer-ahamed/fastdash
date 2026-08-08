// Registry of connector setup tabs shown under Connectors.
//
// Add a connector: write a component taking `ConnectorTabProps` (own state, own
// Save button, persisting only its own config slice via `patchConfig`) and add
// an entry here. The Connectors page renders one sub-tab per entry, in order,
// and the first-run page a card per entry - so `blurbKey` is what a new
// connector says for itself there, with no edit to that page.

import ClaudeConnector from "./ClaudeConnector";
import GithubConnector from "./GithubConnector";
import SentryConnector from "./SentryConnector";
import type { ConnectorTab } from "./types";

export const CONNECTOR_TABS: ConnectorTab[] = [
  {
    id: "claude",
    labelKey: "settings.claude",
    blurbKey: "welcome.claudeBlurb",
    Component: ClaudeConnector,
  },
  {
    id: "github",
    labelKey: "settings.github",
    blurbKey: "welcome.githubBlurb",
    Component: GithubConnector,
  },
  {
    id: "sentry",
    labelKey: "settings.sentry",
    blurbKey: "welcome.sentryBlurb",
    Component: SentryConnector,
  },
];

export type { ConnectorTab, ConnectorTabProps } from "./types";
