import { commands } from "$shared/ipc/bindings";

/** Sites already asked for, by profile, so naming them again asks for nothing. */
const probed = new Set<string>();

/** Asks native for the marks of sites a surface names before they are ever
 *  visited, once each; a private window asks for none. */
export function probe(profile: { id: string; kind: string } | null, urls: readonly string[]) {
  if (!profile || profile.kind === "incognito") return;
  const origins = [
    ...new Set(
      urls.flatMap((url) => {
        try {
          return [new URL(url).origin];
        } catch {
          return [];
        }
      }),
    ),
  ].filter((origin) => !probed.has(`${profile.id} ${origin}`));
  if (!origins.length) return;
  if (probed.size > 512) probed.clear();
  for (const origin of origins) probed.add(`${profile.id} ${origin}`);
  void commands.faviconProbe(profile.id, origins).catch(() => {
    for (const origin of origins) probed.delete(`${profile.id} ${origin}`);
  });
}

export function forgetProbes() {
  probed.clear();
}
