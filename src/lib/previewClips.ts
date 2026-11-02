// The muted preview clips behind Home's hero (Settings › Profiles and Home).
// Each holds a stream from the person's own source, and many accounts allow
// one stream at a time, so anything that really plays stops them first.

const running = new Set<() => void>();

/** Notes a clip that's playing; the function it returns lets it go. */
export function registerPreview(stop: () => void) {
  running.add(stop);
  return () => {
    running.delete(stop);
  };
}

/** Stops every preview clip, before something plays for real. */
export function stopPreviews() {
  const stops = [...running];
  running.clear();
  for (const stop of stops) stop();
}
