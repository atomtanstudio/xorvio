// A pause someone asked for (a button, a key, a media key), as opposed to one
// the web view makes while swapping streams. Pausing a live channel this way
// starts its buffer, when that's how the buffer is set up.

let pausedByPerson = false;

/** Pauses on someone's behalf. */
export function pauseByPerson(video: HTMLVideoElement | null | undefined) {
  if (!video || video.paused) return;
  pausedByPerson = true;
  video.pause();
}

/** Whether the pause that just happened was asked for; asks once. */
export function takePausedByPerson() {
  const asked = pausedByPerson;
  pausedByPerson = false;
  return asked;
}
