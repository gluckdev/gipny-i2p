import { invoke } from './api';

/** The service worker feeds the browser download stream one decrypted part at
 * a time. It never caches the response and has neither keys nor OPFS access.
 * Resolves when the stream finished and rejects on any failure, including one
 * after the download already started (the caller shows it). */
export async function saveBrowserAttachment(attachmentId: number): Promise<void> {
  if (!navigator.serviceWorker) throw new Error('Streaming downloads need the installed browser app');
  let deadline: ReturnType<typeof setTimeout> | undefined;
  const registration = await Promise.race([
    navigator.serviceWorker.ready,
    new Promise<never>((_, reject) => { deadline = setTimeout(() => reject(new Error('Reload the browser app to activate streaming downloads')), 8000); }),
  ]).finally(() => { if (deadline) clearTimeout(deadline); });
  const worker = registration.active;
  if (!worker) throw new Error('Browser download service is not ready');
  const info = await invoke<{ name: string; size: number; parts: number }>('attachment_download_info', { attachmentId });
  const channel = new MessageChannel();
  const token = crypto.randomUUID();
  let index = 0, active = true, busy = false, started = false;
  let idle: ReturnType<typeof setTimeout>;
  let settle: { resolve(): void; reject(error: Error): void } | null = null;
  const finished = new Promise<void>((resolve, reject) => { settle = { resolve, reject }; });
  // The UI may never await `finished` if setup fails; avoid an unhandled rejection.
  finished.catch(() => {});
  const close = (error?: string): void => {
    if (!active) return;
    active = false; clearTimeout(idle); channel.port1.close(); window.removeEventListener('pagehide', pagehide);
    if (error) settle?.reject(new Error(error)); else settle?.resolve();
  };
  const abort = (error = 'Download interrupted'): void => { if (active) channel.port1.postMessage({ error }); close(error); };
  const pagehide = (): void => abort();
  const arm = (): void => { clearTimeout(idle); idle = setTimeout(() => abort('Download timed out'), 120000); };
  window.addEventListener('pagehide', pagehide, { once: true }); arm();
  const ready = new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => { abort('Browser download service did not respond'); reject(new Error('Browser download service did not respond')); }, 8000);
    channel.port1.onmessage = (event): void => {
      if (!active) return;
      arm();
      if (event.data?.ready === token) { clearTimeout(timer); started = true; resolve(); return; }
      if (event.data?.done) { clearTimeout(timer); close(); return; }
      if (event.data?.error || event.data?.cancelled) {
        const message = String(event.data.error ?? 'Download cancelled');
        clearTimeout(timer); close(message);
        if (!started) reject(new Error(message));
        return;
      }
      if (event.data?.pull !== index || busy || index >= info.parts) { abort('Invalid download request'); return; }
      busy = true;
      void invoke<Uint8Array>('attachment_part', { attachmentId, index }).then(part => {
        if (!active) { part.fill(0); return; }
        const bytes = new Uint8Array(part); part.fill(0);
        channel.port1.postMessage({ index: index++, bytes }, [bytes.buffer]);
      }).catch(() => { abort('Attachment is no longer available'); })
        .finally(() => { busy = false; });
    };
  });
  try {
    worker.postMessage({ kind: 'gipny-download', token, name: info.name, size: info.size, parts: info.parts }, [channel.port2]);
    await ready;
    const link = document.createElement('a');
    link.href = new URL(`__gipny_download/${token}`, registration.scope).href;
    link.download = info.name; link.hidden = true;
    document.body.append(link); link.click(); link.remove();
  } catch (error) { abort(); throw error; }
  await finished;
}
