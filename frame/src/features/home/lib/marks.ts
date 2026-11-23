import {
  BlueskyIcon,
  Book02Icon,
  Briefcase01Icon,
  BubbleChatIcon,
  CodeIcon,
  DiscordIcon,
  Facebook01Icon,
  GithubIcon,
  HelpCircleIcon,
  InstagramIcon,
  Linkedin01Icon,
  Mail01Icon,
  MastodonIcon,
  News01Icon,
  NewTwitterIcon,
  PlayIcon,
  RedditIcon,
  Tag01Icon,
  UserGroupIcon,
  Building03Icon,
  Clock01Icon,
  Activity01Icon,
  YoutubeIcon,
} from "@hugeicons/core-free-icons";
import type { IconSvgElement } from "@hugeicons/svelte";
import type { Also, Network, Role } from "./model";

/** A mark for the roles that tell a row apart; a product is most of a site and has none. */
export const ROLE_MARK: Record<Role, IconSvgElement | null> = {
  pricing: Tag01Icon,
  docs: Book02Icon,
  developers: CodeIcon,
  blog: News01Icon,
  help: HelpCircleIcon,
  about: Building03Icon,
  careers: Briefcase01Icon,
  contact: Mail01Icon,
  product: null,
  community: UserGroupIcon,
  changelog: Clock01Icon,
  status: Activity01Icon,
};

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
