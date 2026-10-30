// Allow experimental Linux source builds; reject hosts without a desktop backend.
if (!["darwin", "win32", "linux"].includes(process.platform)) {
  console.error(
    `Zephium does not support ${process.platform}. Use macOS, Windows, or Linux (experimental).`,
  );
  process.exit(1);
}
