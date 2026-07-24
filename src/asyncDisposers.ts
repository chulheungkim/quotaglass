export type Disposer = () => void;

export function collectAsyncDisposers(
  pending: Array<Promise<Disposer>>,
): Disposer {
  let cleanedUp = false;
  const disposers: Disposer[] = [];
  for (const listener of pending) {
    void listener.then((dispose) => {
      if (cleanedUp) {
        dispose();
      } else {
        disposers.push(dispose);
      }
    });
  }
  return () => {
    cleanedUp = true;
    disposers.splice(0).forEach((dispose) => dispose());
  };
}
