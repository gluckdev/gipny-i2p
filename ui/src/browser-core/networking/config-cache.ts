/** Persistence is optional. A broken CacheStorage operation must never hold
 * the online configuration or a profile's configuration barrier indefinitely. */
export async function optionalCache<T>(work: () => Promise<T | undefined>): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([work(), new Promise<undefined>(resolve => { timer = setTimeout(resolve, 250); })]);
  } catch { return undefined; }
  finally { clearTimeout(timer); }
}

/** Settle the caller immediately on lock even if an underlying public cache
 * promise cannot be cancelled. The worker checks the signal before side effects. */
export function whileActive<T>(work: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return work();
  return new Promise((resolve, reject) => {
    const cancel = (): void => reject(new Error('Browser startup cancelled'));
    if (signal.aborted) { cancel(); return; }
    signal.addEventListener('abort', cancel, { once: true });
    work().then(resolve, reject).finally(() => signal.removeEventListener('abort', cancel));
  });
}
