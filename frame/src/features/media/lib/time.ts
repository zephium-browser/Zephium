/** `m:ss`, or `h:mm:ss` from an hour on. */
export function clock(seconds: number): string {
  const whole = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const rest = String(whole % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
}

/** How far through, from 0 to 1; 0 when the length is unknown. */
export function fraction(position: number, duration: number | null): number {
  if (duration === null || !(duration > 0) || !Number.isFinite(position)) return 0;
  return Math.min(1, Math.max(0, position / duration));
}
