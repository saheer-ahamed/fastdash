// Theme registry + controller. The only place that knows about theme ids.
// Color values themselves live in styles.css (one `:root[data-theme="..."]`
// block per theme). To add a theme: add a CSS block there and an entry here.

/** Concrete themes that have a CSS block; excludes the "system" alias. */
export type ResolvedTheme = "dark" | "light" | "midnight" | "amber" | "green" | "paper";
export type ThemeChoice = "system" | ResolvedTheme;

export const THEMES: { id: ThemeChoice; label: string }[] = [
  { id: "system", label: "System" },
  { id: "dark", label: "Dark" },
  { id: "light", label: "Light" },
  { id: "midnight", label: "Midnight" },
  { id: "amber", label: "Amber" },
  { id: "green", label: "Green" },
  { id: "paper", label: "Paper" },
];

const STORAGE_KEY = "fastdash.theme";
const LIGHT_QUERY = "(prefers-color-scheme: light)";

const CHOICES: ThemeChoice[] = THEMES.map((t) => t.id);

export function getStoredTheme(): ThemeChoice {
  const v = localStorage.getItem(STORAGE_KEY) as ThemeChoice | null;
  // Validate against the registry, not a hand-written list: anything narrower
  // silently downgrades the themes it forgot back to "system" on the next
  // launch, which also puts the applied theme at odds with the pre-paint
  // background index.html picks from the same stored value.
  return v && CHOICES.includes(v) ? v : "system";
}

function resolve(choice: ThemeChoice): ResolvedTheme {
  if (choice === "system") {
    return window.matchMedia(LIGHT_QUERY).matches ? "light" : "dark";
  }
  return choice;
}

function apply(choice: ThemeChoice): void {
  document.documentElement.dataset.theme = resolve(choice);
}

/** Persist the choice and apply it instantly. */
export function setTheme(choice: ThemeChoice): void {
  localStorage.setItem(STORAGE_KEY, choice);
  apply(choice);
}

/** Apply the stored choice and keep "system" in sync with OS changes. */
export function initTheme(): void {
  apply(getStoredTheme());
  window.matchMedia(LIGHT_QUERY).addEventListener("change", () => {
    if (getStoredTheme() === "system") apply("system");
  });
}
