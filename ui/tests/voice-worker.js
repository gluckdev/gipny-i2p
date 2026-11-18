// Two real WASM ratchets and OPFS profiles. Only the relay boundary is local.
import init from '../../browser-router/pkg/gipny_browser_router.js';
import { BrowserBackend } from '../src/browser-core/backend';
import { addContact } from '../src/browser-core/messaging/contacts';

const peers = [];
let ciphertexts = 0;
self.onmessage = async ({ data: { key, side, command, frame, lane, attachments } }) => {
  try {
    if (command === 'init') {
      await init();
      for (let i = 0; i < 2; i++) {
        const core = new BrowserBackend((name, payload) => {
          if (name === 'core_event' && payload.CallFrame) self.postMessage({ side: i, event: payload.CallFrame });
        });
        peers.push(core);
        await core.invoke('vault_create', { profile: 'voice' + i, pass: 'only for this fixture', displayName: 'Test' });
      }
      for (const [i, core] of peers.entries()) {
        const other = peers[1 - i];
        const identity = other.identity;
        await core.transact(state => addContact(state, core.identity, core.log, {
          name: 'Test', signPk: identity.sign_pk, dhPk: identity.dh_pk, i2pAddress: identity.i2p_address,
        }, 'none'));
        core.relay = {
          bundle: async () => other.engine.my_bundle(),
          deposit: async (_address, recipient, bytes) => {
            if (recipient !== identity.sign_pk || new TextDecoder().decode(bytes).includes('gipny-call/1')) throw Error('Unexpected relay payload');
            ciphertexts++;
            await other.receive(core.identity.sign_pk, bytes);
          },
          close() {},
        };
      }
    } else if (command === 'send') {
      const core = peers[side];
      await core.invoke('send_call_frame', { contactId: core.state.contacts[0].id, frame, lane });
    } else if (command === 'sendClip') {
      const core = peers[side];
      await core.invoke('send_message', { contactId: core.state.contacts[0].id, body: '', attachments });
    } else if (command === 'receivedClip') {
      const core = peers[side];
      const file = core.state.attachments[0];
      const value = file ? { name: file.name, data: await core.invoke('load_attachment', { attachmentId: file.id }) } : null;
      self.postMessage({ key, value });
      return;
    } else if (command === 'report') {
      self.postMessage({ key, value: { ciphertexts, messages: peers.map(core => core.state.messages.length), outbox: peers.map(core => core.state.outbox.length) } });
      return;
    }
    self.postMessage({ key });
  } catch (error) { self.postMessage({ key, error: String(error) }); }
};
