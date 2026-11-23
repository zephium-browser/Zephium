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
import type { Also, Network } from "./model";

export const NETWORK_MARK: Record<Network, IconSvgElement> = {
  x: NewTwitterIcon,
  github: GithubIcon,
  youtube: YoutubeIcon,
  linkedin: Linkedin01Icon,
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
