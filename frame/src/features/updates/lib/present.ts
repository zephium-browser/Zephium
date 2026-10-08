import type { IconSvgElement } from "@hugeicons/svelte";
import {
  Alert02Icon,
  Bug01Icon,
  News01Icon,
  RefreshIcon,
  Shield01Icon,
  SparklesIcon,
  SystemUpdate01Icon,
} from "@hugeicons/core-free-icons";
import * as m from "$shared/i18n/messages";
import { commands } from "$shared/ipc/bindings";
import { IS_MAC } from "$shared/platform";
import type { SidebarCardAction } from "$shared/ui/SidebarCard";
import { releaseNotesUrl, updates } from "$domain/updates";
import * as notices from "./notices.svelte";
import type { UpdateCard, UpdatePill } from "./select";

const REPORT_URL = "https://github.com/zephium-browser/Zephium/issues/new?template=bug_report.yml";

export type CardView = {
  key: string;
  title: string;
  detail?: string;
  items?: string[];
  icon: IconSvgElement;
  actions: SidebarCardAction[];
  dismiss: () => void;
};

export function cardView(card: UpdateCard): CardView {
  if (card.kind === "session") {
    return {
      key: "session",
      title: m.session_set_aside_title(),
      detail: m.session_set_aside_detail(),
      icon: Alert02Icon,
      actions: [],
      dismiss: notices.dismissSession,
    };
  }
  if (card.kind === "updated") {
    return {
      key: `updated:${card.version}`,
      title: m.update_done_title({ version: card.version }),
      items: notices.highlights(card.version),
      icon: SparklesIcon,
      actions: [
        {
          label: m.update_whats_new(),
          icon: News01Icon,
          dismisses: true,
          onclick: () =>
            void commands.browserOpenUrl(releaseNotesUrl(card.version), true).catch(() => {}),
        },
        {
          label: m.update_report_problem(),
          icon: Bug01Icon,
          onclick: () => void commands.browserOpenUrl(REPORT_URL, true).catch(() => {}),
        },
      ],
      dismiss: notices.acknowledgeUpdate,
    };
  }
  if (card.target === "browser_runtime") {
    return {
      key: "security:browser_runtime",
      title: m.update_webview_title(),
      detail: m.update_webview_detail(),
      icon: Shield01Icon,
      actions: [],
      dismiss: notices.dismissSecurity,
    };
  }
  return {
    key: "security:operating_system",
    title: IS_MAC ? m.update_security_mac_title() : m.update_security_system_title(),
    detail: m.update_security_detail(),
    icon: Shield01Icon,
    actions: IS_MAC
      ? [
          {
            label: m.update_security_open(),
            icon: SystemUpdate01Icon,
            onclick: () => void commands.openSoftwareUpdate().catch(() => {}),
          },
        ]
      : [],
    dismiss: notices.dismissSecurity,
  };
}

export const PILL_ICON = RefreshIcon;

export const pillLabel = (pill: UpdatePill) =>
  pill.kind === "manual"
    ? m.update_install_manual()
    : pill.kind === "ready"
      ? m.update_relaunch()
      : m.update_installing();

export function activatePill(pill: UpdatePill) {
  if (pill.kind === "manual")
    void commands.browserOpenUrl(releaseNotesUrl(pill.version), true).catch(() => {});
  else if (pill.kind === "ready") void updates.relaunch();
}
