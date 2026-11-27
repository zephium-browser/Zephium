import * as m from "$shared/i18n/messages";
import { preferences } from "$domain/preferences";
import { fields } from "./catalog";
import { IS_WINDOWS } from "$shared/platform";
import {
  Settings01Icon,
  PaintBoardIcon,
  Add01Icon,
  Layers01Icon,
  UserCircleIcon,
  Search01Icon,
  Shield01Icon,
  KeyboardIcon,
  Globe02Icon,
  ChipIcon,
  Briefcase02Icon,
  Clock01Icon,
  DashboardSpeed01Icon,
  InformationCircleIcon,
  UserMultiple02Icon,
  Mortarboard02Icon,
  Brain01Icon,
  Plug01Icon,
  DashboardSquare02Icon,
} from "@hugeicons/core-free-icons";
/** Navigation order: groups in `groups` order, sections within a group as
 *  listed here. Ids are deep-link targets, so a rename changes titles only. */
const allSections = [
  {
    id: "general",
    group: "browser",
    title: m.section_general,
    description: m.section_general_description,
    icon: Settings01Icon,
  },
  {
    id: "appearance",
    group: "browser",
    title: m.section_appearance,
    description: m.section_appearance_description,
    icon: PaintBoardIcon,
  },
  {
    id: "tabs",
    group: "browser",
    title: m.section_tabs,
    description: m.section_tabs_description,
    icon: Layers01Icon,
  },
  {
    id: "newtab",
    group: "browser",
    title: m.section_newtab,
    description: m.section_newtab_description,
    icon: Add01Icon,
  },
  {
    id: "search",
    group: "browser",
    title: m.section_search,
    description: m.section_search_description,
    icon: Search01Icon,
  },
  {
    id: "privacy",
    group: "browser",
    title: m.section_privacy,
    description: m.section_privacy_description,
    icon: Shield01Icon,
  },
  {
    id: "focus",
    group: "browser",
    title: m.section_focus,
    description: m.section_focus_description,
    icon: Clock01Icon,
  },
  {
    id: "performance",
    group: "browser",
    title: m.section_performance,
    description: m.section_performance_description,
    icon: DashboardSpeed01Icon,
  },
  {
    id: "profiles",
    group: "browser",
    title: m.section_profiles,
    description: m.section_profiles_description,
    icon: UserMultiple02Icon,
  },
  {
    id: "work",
    group: "work",
    title: m.section_work,
    description: m.ai_switch_section_description,
    icon: Briefcase02Icon,
  },
  {
    id: "ai",
    group: "work",
    title: m.section_ai,
    description: m.section_ai_description,
    icon: ChipIcon,
  },
  {
    id: "skills",
    group: "work",
    title: m.section_skills,
    description: m.section_skills_description,
    icon: Mortarboard02Icon,
  },
  {
    id: "memory",
    group: "work",
    title: m.section_memory,
    description: m.section_memory_description,
    icon: Brain01Icon,
  },
  {
    id: "sites",
    group: "work",
    title: m.section_sites,
    description: m.section_sites_description,
    icon: Globe02Icon,
  },
  {
    id: "mcp",
    group: "work",
    title: m.section_mcp,
    description: m.section_mcp_description,
    icon: Plug01Icon,
  },
  {
    id: "features",
    group: "app",
    title: m.section_features,
    description: m.section_features_description,
    icon: DashboardSquare02Icon,
  },
  {
    id: "shortcuts",
    group: "app",
    title: m.section_shortcuts,
    description: m.section_shortcuts_description,
    icon: KeyboardIcon,
  },
  {
    id: "about",
    group: "app",
    title: m.section_about,
    description: m.section_about_description,
    icon: InformationCircleIcon,
  },
  // Reached through the identity card above the groups, and through search.
  {
    id: "account",
    group: "identity",
    title: m.section_account,
    description: m.section_account_description,
    icon: UserCircleIcon,
  },
] as const;
export type SettingsSection = (typeof allSections)[number]["id"];
/** Placeholder settings, made to review a design before it works, appear
 *  only in development builds. A release offers only what works. */
export const showPreviews = import.meta.env.DEV;
/** Sections that hold nothing but placeholders today. */
const previewSections = new Set<SettingsSection>();
export const sections = allSections.filter(
  (section) => showPreviews || !previewSections.has(section.id),
);
/** Pages that leave with their feature's switch. */
const featureSections: Partial<Record<SettingsSection, () => boolean>> = {
  work: () => preferences.enabled("work"),
  focus: () => preferences.enabled("time"),
  ai: preferences.aiEnabled,
  skills: preferences.aiEnabled,
  memory: preferences.aiEnabled,
  sites: preferences.aiEnabled,
  mcp: preferences.aiEnabled,
};
export const shownSections = () =>
  sections.filter((section) => featureSections[section.id]?.() ?? true);
export const groups = [
  { id: "browser", label: m.settings_browser },
  { id: "work", label: m.section_work },
  { id: "app", label: m.settings_application },
] as const;
export function searchSettings(query: string) {
  const words = query.trim().toLocaleLowerCase().split(/\s+/u);
  const matches = (text: string) => words.every((word) => text.toLocaleLowerCase().includes(word));
  const offered = shownSections();
  const shown = new Set<string>(offered.map((section) => section.id));
  const preferences = Object.entries(fields)
    .filter(
      ([, field]) =>
        shown.has(field.section) &&
        (showPreviews || !("preview" in field)) &&
        (!("windowsOnly" in field) || IS_WINDOWS) &&
        matches(field.label() + " " + field.description()),
    )
    .map(([id, field]) => ({
      id,
      section: field.section,
      label: field.label,
      description: field.description,
      target: id as string | null,
    }));
  const destinations = offered
    .filter(
      (section) =>
        matches(section.title()) && !preferences.some((field) => field.section === section.id),
    )
    .map((section) => ({
      id: `section:${section.id}`,
      section: section.id,
      label: section.title,
      description: section.description,
      target: null as string | null,
    }));
  return [...destinations, ...preferences];
}
export function searchSections(query: string) {
  const words = query.trim().toLocaleLowerCase().split(/\s+/u);
  return shownSections().filter((section) =>
    words.every((word) =>
      [
        section.title(),
        section.description(),
        ...Object.values(fields)
          .filter(
            (f) =>
              f.section === section.id &&
              (showPreviews || !("preview" in f)) &&
              (!("windowsOnly" in f) || IS_WINDOWS),
          )
          .flatMap((f) => [f.label(), f.description()]),
      ]
        .join(" ")
        .toLocaleLowerCase()
        .includes(word),
    ),
  );
}
