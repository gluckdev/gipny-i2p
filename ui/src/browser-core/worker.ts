import init from '../../../browser-router/pkg/gipny_browser_router.js';
import { BrowserBackend } from './backend';

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent) => void) | null;
  postMessage(message: unknown): void;
};
const backend = new BrowserBackend((event, payload) => scope.postMessage({ event, payload }));
const ready = init();
scope.onmessage = (event: MessageEvent): void => {
  const request = event.data as { id?: unknown; command?: unknown; args?: unknown };
  if (!Number.isSafeInteger(request.id) || typeof request.command !== 'string' || request.command.length > 100 || (request.args !== undefined && (!request.args || typeof request.args !== 'object' || Array.isArray(request.args)))) return;
  const respond = async (): Promise<void> => {
    try {
      await ready;
      const result = await backend.invoke(request.command as string, request.args as Record<string, unknown> | undefined);
      scope.postMessage({ id: request.id, result });
    } catch (error) {
      scope.postMessage({ id: request.id, error: String(error instanceof Error ? error.message : error).slice(0, 1000) });
    }
  };
  // No global RPC queue: vault_lock must interrupt pending network startup.
  // The backend serializes only its short storage/crypto transactions.
  void respond();
};
