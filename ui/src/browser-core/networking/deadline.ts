/** A live relay answers within seconds once a stream is open. One silent this
 * long sits on a dead stream (often a reused one): giving up lets the letter go
 * again on a fresh stream instead of waiting out the transport's write and
 * read timeouts, which add up to minutes. */
export const RELAY_ANSWER_MS = 60_000;

export function answerWithin<T>(work: Promise<T>, ms = RELAY_ANSWER_MS): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Relay did not answer in time')), ms); });
  return Promise.race([work, expired]).finally(() => clearTimeout(timer));
}
