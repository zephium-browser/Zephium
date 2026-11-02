import chatgpt from "../marks/chatgpt.svg";
import chatgptDark from "../marks/chatgpt-dark.svg";
import claude from "../marks/claude.svg";
import figma from "../marks/figma.svg";
import github from "../marks/github.svg";
import githubDark from "../marks/github-dark.svg";
import gmail from "../marks/gmail.svg";
import googleCalendar from "../marks/google-calendar.svg";
import linear from "../marks/linear.svg";
import notion from "../marks/notion.svg";
import slack from "../marks/slack.svg";
import spotify from "../marks/spotify.svg";
import whatsapp from "../marks/whatsapp.svg";
import youtube from "../marks/youtube.svg";

import type { BrandArt } from "$shared/ui/BrandMark";

export type Mark = BrandArt;

/** A site onboarding offers to keep. Native owns each one's address and the
 *  raster it seeds; chrome names it by id and draws it from here. The lines
 *  below are read by `scripts/kept-site-rasters.mjs` and by native's drift
 *  test, so keep one site per line in this shape. */
export type Site = { id: string; name: string; mark: Mark };

export const SITES: readonly Site[] = [
  { id: "gmail", name: "Gmail", mark: { light: gmail } },
  { id: "google-calendar", name: "Calendar", mark: { light: googleCalendar } },
  { id: "slack", name: "Slack", mark: { light: slack } },
  { id: "notion", name: "Notion", mark: { light: notion } },
  { id: "linear", name: "Linear", mark: { light: linear } },
  { id: "github", name: "GitHub", mark: { light: github, dark: githubDark } },
  { id: "figma", name: "Figma", mark: { light: figma } },
  { id: "chatgpt", name: "ChatGPT", mark: { light: chatgpt, dark: chatgptDark } },
  { id: "claude", name: "Claude", mark: { light: claude } },
  { id: "youtube", name: "YouTube", mark: { light: youtube } },
  { id: "spotify", name: "Spotify", mark: { light: spotify } },
  { id: "whatsapp", name: "WhatsApp", mark: { light: whatsapp } },
];

/** The address native keeps for each site, so chrome can tell which of them
 *  are already in Essentials from the projection alone. Must match native's
 *  catalog exactly; its drift test compares the two. */
export const SITE_URLS: Readonly<Record<string, string>> = {
  gmail: "https://mail.google.com/",
  "google-calendar": "https://calendar.google.com/",
  slack: "https://app.slack.com/client",
  whatsapp: "https://web.whatsapp.com/",
  notion: "https://www.notion.so/",
  linear: "https://linear.app/",
  github: "https://github.com/",
  figma: "https://www.figma.com/files",
  chatgpt: "https://chatgpt.com/",
  claude: "https://claude.ai/",
  youtube: "https://www.youtube.com/",
  spotify: "https://open.spotify.com/",
};

/** The browsers people most often come from, in the order they are offered
 *  until native has looked for what is actually installed. */
export const BROWSERS: readonly (readonly [string, string])[] = [
  ["arc", "Arc"],
  ["zen", "Zen"],
  ["chrome", "Chrome"],
  ["safari", "Safari"],
  ["brave", "Brave"],
  ["firefox", "Firefox"],
  ["edge", "Edge"],
];
