import {
  BlueskyIcon,
  BubbleChatIcon,
  DiscordIcon,
  Facebook01Icon,
  GithubIcon,
  InstagramIcon,
  Linkedin01Icon,
  MastodonIcon,
  NewTwitterIcon,
  PlayIcon,
  RedditIcon,
  YoutubeIcon,
} from "@hugeicons/core-free-icons";
import type { IconSvgElement } from "@hugeicons/svelte";
import type { Also } from "$domain/home";
import type { MapNetwork } from "$shared/ipc/bindings";

export const NETWORK_MARK: Record<MapNetwork, IconSvgElement> = {
  twitter: NewTwitterIcon,
  git_hub: GithubIcon,
  you_tube: YoutubeIcon,
  linked_in: Linkedin01Icon,
  discord: DiscordIcon,
  reddit: RedditIcon,
  mastodon: MastodonIcon,
  bluesky: BlueskyIcon,
  instagram: InstagramIcon,
  facebook: Facebook01Icon,
};

export const KIND_MARK: Record<Also["kind"], IconSvgElement | null> = {
  page: null,
  discussion: BubbleChatIcon,
  video: PlayIcon,
};

/** Each network by the name it goes by. */
export const NETWORK_NAME: Record<MapNetwork, string> = {
  twitter: "X",
  git_hub: "GitHub",
  you_tube: "YouTube",
  linked_in: "LinkedIn",
  discord: "Discord",
  reddit: "Reddit",
  mastodon: "Mastodon",
  bluesky: "Bluesky",
  instagram: "Instagram",
  facebook: "Facebook",
};
