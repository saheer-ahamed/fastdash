// Config read/write helpers shared by Settings and the Connectors tabs.
//
// Every section (General, and each connector) saves independently, so a save
// must never write back a stale copy of the sections it does not own. Each
// `patchConfig` call re-reads the persisted config, overlays only the caller's
// slice, and writes that - so two sections saved in any order both survive.

import { invoke } from "@tauri-apps/api/core";
import type { AppConfig } from "./types";

export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

export async function patchConfig(patch: Partial<AppConfig>): Promise<AppConfig> {
  const current = await getConfig();
  const next: AppConfig = { ...current, ...patch };
  await invoke("save_config", { config: next });
  return next;
}
