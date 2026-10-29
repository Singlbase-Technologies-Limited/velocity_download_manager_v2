/** The question before quitting stops active downloads (V1's confirm exit). */
export function quitQuestion(active: number) {
  const one = active === 1;
  return {
    title: 'Quit Velocity?',
    description: `${String(active)} ${one ? 'download is' : 'downloads are'} still running. Quitting stops ${one ? 'it' : 'them'}. Unfinished downloads wait in the queue and continue from where they stopped, when they can, the next time Velocity starts.`,
    confirmLabel: 'Quit',
    cancelLabel: 'Keep downloading',
  };
}
