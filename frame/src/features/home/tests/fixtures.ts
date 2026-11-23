import { fromSnapshot, type HomeModel, type Refusal } from "$domain/home";
import { snapshot } from "$shared/testing/home-wire";
import {
  HOME_TABS,
  NOW,
  arriving,
  bare,
  busyWire,
  compare,
  failed,
  figma,
  figmaLocal,
  large,
  long,
  raycast,
  rtl,
  studio,
  todayWire,
  type Wire,
} from "$shared/testing/home-scenes";

export * from "$shared/testing/home-scenes";

type Held = {
  unfolded?: ReadonlySet<string>;
  waiting?: ReadonlySet<string>;
  refused?: ReadonlyMap<string, Refusal>;
};

/** Home's model from native searches, newest first, as the domain derives it. */
export const home = (searches: Wire[], held: Held = {}): HomeModel =>
  fromSnapshot(snapshot(searches), { tabs: HOME_TABS, ...held }, NOW);

/** A full Home: a day of twenty-two searches and two days before it, thirty-two in all. */
export const busy = (): HomeModel => home(busyWire());

export const today = (): HomeModel => home(todayWire());

export const states = (): HomeModel => home([arriving(), figma(), failed()], figmaLocal);

export const edges = (): HomeModel => home([long(), rtl()]);

export const rtlFirst = (): HomeModel => home([rtl(), long()]);

export const thinPlaces = (): HomeModel => home([studio(), bare(), raycast()]);

export const comparing = (): HomeModel => home([compare(0), raycast()]);

/** Stripe's large site with its documentation unfolded, as the person left it. */
export const largeOpen = (): HomeModel =>
  home([large()], { unfolded: new Set(["q-stripe/documentation"]) });
