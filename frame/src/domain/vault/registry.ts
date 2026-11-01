import { VaultSession } from "./vault.svelte";

const sessions = new Map<string, VaultSession>();

/** One session per profile for the life of the document. */
export function vaultSession(profile: string): VaultSession {
  let session = sessions.get(profile);
  if (!session) {
    session = new VaultSession(profile);
    sessions.set(profile, session);
    void session.start();
  }
  return session;
}
