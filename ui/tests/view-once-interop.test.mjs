// Build browser-router/pkg with wasm-pack first, then run from ui with
// node --test tests/view-once-interop.test.mjs. Both endpoints use real crypto.
// GIPNY_VIEW_ONCE_PEER may point at a previously built host view_once_peer.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from 'vite';

// I2P is exercised by run_messenger_ci.py. These two byte streams keep the
// real relay protocol, AuthV2, WASM frame codecs and both production endpoints.
function streamPair(observeClientFrame, observeServerFrame = () => {}) {
  let closed = false;
  const pipes = [{ queued: [], reader: null }, { queued: [], reader: null }];
  return pipes.map((own, side) => ({
    async read_frame() {
      if (own.queued.length) return own.queued.shift();
      if (closed) throw Error('stream closed');
      return new Promise((resolve, reject) => { own.reader = { resolve, reject }; });
    },
    async write_frame(bytes) {
      if (closed) throw Error('stream closed');
      if (side === 0) observeClientFrame(bytes); else observeServerFrame(bytes);
      const other = pipes[1 - side];
      if (other.reader) { other.reader.resolve(bytes); other.reader = null; }
      else other.queued.push(bytes);
    },
    close() {
      closed = true;
      for (const pipe of pipes) { pipe.reader?.reject(Error('stream closed')); pipe.reader = null; }
    },
    free() {},
  }));
}

test('native and browser view-once letters interoperate, consume once and keep only replay markers on restart', async t => {
  const repo = fileURLToPath(new URL('../../', import.meta.url));
  const wasm = new URL('../../browser-router/pkg/gipny_browser_router.js', import.meta.url);
  const dir = await mkdtemp(join(tmpdir(), 'gipny-view-once-'));
  const binary = process.env.GIPNY_VIEW_ONCE_PEER;
  const env = { ...process.env, I2P_EMBED_STUB: '1' };
  // The example runs locally even in this project's Android build shell.
  for (const key of ['CC', 'CXX', 'AR', 'RANLIB', 'CARGO_BUILD_TARGET']) if (/android/i.test(env[key] || '')) delete env[key];
  const native = spawn(binary ? resolve(binary) : 'cargo', binary ? [] : ['run', '--locked', '--quiet', '-p', 'gipny-libcore', '--example', 'view_once_peer'], {
    cwd: repo, env, stdio: ['pipe', 'pipe', 'inherit'],
  });
  const lines = createInterface({ input: native.stdout })[Symbol.asyncIterator]();
  const reply = async () => {
    const line = await lines.next();
    if (line.done) throw Error('Native test peer stopped before replying');
    return JSON.parse(line.value);
  };
  const call = args => { native.stdin.write(JSON.stringify(args) + '\n'); return reply(); };
  const pass = 'synthetic test profile passphrase';
  let backend;
  try {
    await build({ configFile: false, logLevel: 'silent', plugins: [{ name: 'test-real-wasm', enforce: 'pre',
      resolveId(id) { if (id.endsWith('/pkg/gipny_browser_router.js')) return { id: wasm.href, external: true }; },
    }], build: { outDir: dir, emptyOutDir: false,
      lib: { entry: 'tests/profile-state-entry.ts', formats: ['es'], fileName: () => 'entry.mjs' },
    } });
    const { BrowserBackend, BrowserEngine, BrowserRelay, newState, viewOnceParts, ViewOnceParts } = await import(pathToFileURL(join(dir, 'entry.mjs')));
    const { default: init, decode_client_frame, decode_relay_frame } = await import(wasm);
    await init({ module_or_path: await readFile(new URL('./gipny_browser_router_bg.wasm', wasm)) });
    const peer = await reply();
    const engine = BrowserEngine.create('interop', pass);
    const browser = JSON.parse(engine.public_info());
    const browserCard = { sign_pk: browser.sign_pk, dh_pk: browser.dh_pk };
    const auth = { address: browser.i2p_address, challenge: [...crypto.getRandomValues(new Uint8Array(32))], nonce: [...crypto.getRandomValues(new Uint8Array(32))] };
    const challengeBytes = Uint8Array.from(auth.challenge), nonceBytes = Uint8Array.from(auth.nonce);
    const proof = engine.terminal_proof(peer.sign_pk, challengeBytes, nonceBytes);
    assert.deepEqual(await call({ op: 'verify_terminal_proof', peer: browserCard, ...auth, proof: [...proof] }), { verified: true });
    assert.deepEqual(await call({ op: 'verify_terminal_proof', peer: browserCard, ...auth, nonce: auth.nonce.map(i => i ^ 1), proof: [...proof] }), { verified: false });
    const nativeProof = await call({ op: 'issue_terminal_proof', peer: browserCard, ...auth });
    assert.ok(BrowserEngine.verify_terminal_proof(peer.sign_pk, auth.address, browser.sign_pk, challengeBytes, nonceBytes, Uint8Array.from(nativeProof.proof)));
    const base = newState('test');
    base.contacts.push({ ...peer, id: 1, request: 'none', trust: 1, i2p_address: 'peer.i2p' });
    let snapshot;
    const events = [];
    function attach(openEngine, state) {
      const core = new BrowserBackend((event, value) => { if (event === 'core_event') events.push(value); });
      core.engine = openEngine; core.identity = browser; core.state = state;
      core.storage = { close() {}, persist: async (sealed, _sequence, mirror) => { assert.ok(mirror); snapshot = sealed.slice(); } };
      return core;
    }
    backend = attach(engine, structuredClone(base));
    const params = () => ({ id: [...crypto.getRandomValues(new Uint8Array(16))], nonce: [...crypto.getRandomValues(new Uint8Array(32))], expires_at: Date.now() + 300_000 });
    const first = params();
    const { offer } = await call({ op: 'issue', peer: browserCard, ...first });
    const body = '🔑'.repeat(4096); // The 16 KiB UTF-8 boundary requires five encrypted parts.
    const seal = nonce => engine.view_once_seal(peer.sign_pk, peer.dh_pk, new Uint8Array(first.id), new Uint8Array(nonce), first.expires_at, new Uint8Array(offer), new TextEncoder().encode(body));
    assert.throws(() => seal(first.nonce.map(n => n ^ 1)), 'a signed offer cannot be reused for another challenge');
    const outbound = seal(first.nonce);
    const parts = [...viewOnceParts(outbound)];
    assert.equal(parts.length, 5);
    for (const part of parts) {
      assert.deepEqual(await call({ op: 'part', ...part }), part.index + 1 === part.total ? { accepted: true } : { ack: part.index });
    }
    assert.deepEqual(await call({ op: 'open' }), { body });
    assert.deepEqual(await call({ op: 'open' }), { error: true });

    const receive = async (req, text, corrupt = false) => {
      const incoming = backend.receiveViewOnceOffer(peer.sign_pk, { ...req, to: [...Buffer.from(browser.sign_pk, 'hex')] });
      try {
        const { parts } = await call({ op: 'seal_parts', peer: browserCard, ...req, offer: [...incoming.bytes], body: text });
        assert.ok(parts.length >= 2);
        if (corrupt) parts.at(-1).bytes[0] ^= 1;
        const assembly = new ViewOnceParts();
        try {
          for (const part of parts) {
            const envelope = assembly.add(part);
            if (envelope) await incoming.accept(envelope);
          }
        } finally { assembly.clear(); }
      } finally { incoming.close(); }
    };
    const good = params(), unopened = params(), invalid = params();
    const key = req => Buffer.from(req.id).toString('hex');
    await assert.rejects(receive(invalid, 'tamper check', true));
    assert.deepEqual(events, []);
    await receive(good, 'native to WASM • 🔑');
    await receive(unopened, 'unopened • 🔒');
    const publicNotices = await backend.invoke('list_view_once', { contactId: 1 });
    assert.deepEqual(publicNotices.map(n => n.id), [key(good), key(unopened)]);
    for (const item of publicNotices) assert.deepEqual(Object.keys(item).sort(), ['contact_id', 'expires_at', 'id', 'sent_at']);
    assert.deepEqual(events, publicNotices.map(info => ({ ViewOnceAvailable: info })));
    assert.deepEqual(await backend.invoke('list_messages', { contactId: 1 }), []);
    const opens = await Promise.allSettled([0, 1].map(() => backend.invoke('open_view_once', { id: key(good) })));
    assert.deepEqual(opens.filter(x => x.status === 'fulfilled').map(x => x.value), ['native to WASM • 🔑']);
    assert.equal(opens.filter(x => x.status === 'rejected').length, 1);
    backend.closeNow();

    const fresh = BrowserEngine.open_password('interop', pass, snapshot, new Uint8Array());
    const restored = JSON.parse(fresh.take_metadata());
    const { outbox, view_once_tracking: tracking, ...withoutReceipts } = restored;
    const markers = Object.fromEntries([good, unopened].map(req => [key(req), { peer: peer.sign_pk, expires_at: req.expires_at }]));
    assert.deepEqual({ ...withoutReceipts, outbox: [] }, { ...base, view_once_seen: markers });
    assert.deepEqual(tracking.deliveries, []);
    assert.deepEqual(Object.keys(tracking.receipts).sort(), [key(good), key(unopened)].sort());
    assert.deepEqual(outbox.map(row => row.payload.callback_data).sort(),
      [good, unopened].map(req => 'gipny.viewonce.received.v1/' + key(req)).sort());
    for (const row of outbox) {
      assert.equal(row.blob, null); assert.equal(row.payload.body, ''); assert.deepEqual(row.payload.attachments, []); assert.deepEqual(row.payload.files, []);
    }
    for (const req of [good, unopened]) {
      const claim = tracking.receipts[key(req)];
      assert.deepEqual(Object.keys(claim).sort(), ['deadline', 'next', 'peer']);
      assert.equal(claim.peer, peer.sign_pk); assert.equal(claim.deadline, req.expires_at);
    }
    backend = attach(fresh, restored);
    assert.deepEqual(await backend.invoke('list_view_once', { contactId: 1 }), []);
    await assert.rejects(backend.invoke('open_view_once', { id: key(unopened) }));
    const already = backend.receiveViewOnceOffer(peer.sign_pk, { ...unopened, to: [...Buffer.from(browser.sign_pk, 'hex')] });
    await already.recorded;
    assert.equal(already.bytes, undefined);
    await assert.rejects(backend.invoke('open_view_once', { id: key(unopened) }));
    assert.equal(events.length, 2);

    await t.test('offline invoke retains only delivery metadata and a lost final ACK never releases a second letter', async () => {
      const senderName = 'queued sender', receiverName = 'queued receiver';
      const wait = async (check, label) => {
        const end = Date.now() + 12000;
        while (!check()) {
          if (Date.now() >= end) assert.fail(typeof label === 'function' ? label() : label);
          await new Promise(resolve => setTimeout(resolve, 10));
        }
      };
      function endpoint(name) {
        const engine = BrowserEngine.create(name, pass);
        const card = JSON.parse(engine.public_info());
        const events = [], state = newState(name);
        const snapshots = [];
        const core = new BrowserBackend((event, value) => { if (event === 'core_event') events.push(value); });
        core.engine = engine; core.identity = card; core.state = state;
        core.storage = { close() {}, async persist(bytes) { snapshots.push(bytes.slice()); } };
        return { core, engine, card, state, events, snapshots };
      }
      function storedState(name, endpoint) {
        const reopened = BrowserEngine.open_password(name, pass, endpoint.snapshots.at(-1), new Uint8Array());
        try { return JSON.parse(reopened.take_metadata()); } finally { reopened.close(); reopened.free(); }
      }
      const alice = endpoint(senderName), bob = endpoint(receiverName);
      const contact = ({ card }) => ({ ...card, id: 1, request: 'none', trust: 1 });
      alice.state.contacts.push(contact(bob)); bob.state.contacts.push(contact(alice));
      const before = structuredClone(alice.state);
      const toBob = [], reverseFrames = [], errors = [], tasks = new Set();
      let online = false, offlineAttempts = 0, loseAck = true, closedCopy;
      function network(target, forward) {
        return { async connect(address) {
          assert.equal(address, target.card.i2p_address);
          if (!online) { offlineAttempts++; throw Error('Peer is offline'); }
          const [client, server] = streamPair(bytes => {
            const frame=JSON.parse(decode_client_frame(bytes));
            if (forward) toBob.push(frame); else reverseFrames.push(Object.keys(frame)[0]);
          }, bytes => {
            const frame = JSON.parse(decode_relay_frame(bytes));
            if (forward && loseAck && frame.ViewOnceStoredV1) {
              loseAck = false; server.close(); throw Error('Lost final transfer acknowledgment');
            }
          });
          const task = target.relay.serve(server).catch(err => { errors.push(String(err)); server.close(); });
          tasks.add(task); void task.finally(() => tasks.delete(task));
          return client;
        }, close() {} };
      }
      for (const [self, remote] of [[alice,bob],[bob,alice]]) {
        self.relay = new BrowserRelay(self.engine, network(remote,self === alice), self.card,
          (peer, blob) => self.core.receive(peer,blob), () => {}, () => {},
          (peer, req) => self.core.receiveViewOnceOffer(peer,req));
      }
      try {
        alice.core.relay = alice.relay;
        const body = 'volatile-only ★ '.repeat(700);
        const id = await alice.core.invoke('send_view_once', { contactId: 1, ttlSecs: 300, body });
        await wait(() => offlineAttempts > 0, 'the sender did not try the offline recipient');
        const metadata = storedState(senderName,alice);
        const record = metadata.view_once_tracking.deliveries[0];
        assert.deepEqual(metadata, { ...before,view_once_tracking: { deliveries:[{id,contact_id:1,deadline:record.deadline,status:'pending'}], receipts:{} } });
        const entry = alice.core.viewOnceOutbox.letters.get(id);
        assert.ok(entry.key.length > 0 && entry.parts.length > 1);
        const parts = entry.parts, wrapped = entry.key;
        online = true;
        await wait(() => !entry.sending, 'offline attempt did not finish');
        alice.core.resumePeerRetries(bob.card.sign_pk,alice.core.generation);
        await wait(() => !loseAck && !entry.sending, 'receiver did not store the fragments before the final ACK was lost');
        assert.equal((await bob.core.invoke('list_view_once', { contactId: 1 }))[0].id,id);
        assert.equal(await bob.core.invoke('open_view_once', { id }),body);
        assert.equal((await alice.core.invoke('list_view_once_deliveries'))[0].status,'pending');
        const uploads = toBob.filter(frame => frame.ViewOncePartV1).length;
        assert.ok(uploads > 1);
        // Even with its receipt already acknowledged, the recipient must
        // recognize which sender may replay this exact accepted offer.
        await bob.core.transact(state => { delete state.view_once_tracking.receipts[id]; state.outbox = []; },true);
        const prior = toBob.find(frame => frame.ViewOnceOfferV1).ViewOnceOfferV1;
        await bob.core.transact(state => state.contacts.push({ ...peer,id:2,request:'none',trust:1 }),true);
        assert.throws(() => bob.core.receiveViewOnceOffer(peer.sign_pk, prior));
        assert.throws(() => bob.core.receiveViewOnceOffer(alice.card.sign_pk, { ...prior, expires_at: prior.expires_at+1 }));
        alice.core.resumePeerRetries(bob.card.sign_pk,alice.core.generation);
        await wait(() => alice.core.state.view_once_tracking.deliveries[0].status === 'relayed', 'duplicate offer was not acknowledged');
        assert.equal(toBob.filter(frame => frame.ViewOnceOfferV1).length, 2);
        assert.equal(toBob.filter(frame => frame.ViewOncePartV1).length, uploads);
        assert.equal(bob.events.filter(event => event.ViewOnceAvailable).length,1);
        assert.equal(bob.core.state.view_once_tracking.receipts[id].peer,alice.card.sign_pk);
        await assert.rejects(bob.core.invoke('open_view_once',{id}));
        assert.ok(wrapped.every(b => b===0) && parts.every(part => part.every(b=>b===0)));
        bob.core.relay = bob.relay;
        bob.core.schedule(0);
        await wait(() => alice.core.state.view_once_tracking.deliveries[0].status === 'received',
          () => 'Ratchet receipt did not arrive. ' + JSON.stringify({ reverseFrames, errors }));
        online = false; alice.relay.close(); alice.core.relay=null;
        const expired = await alice.core.invoke('send_view_once', { contactId: 1, ttlSecs: 0, body: 'zero TTL' });
        assert.equal(alice.core.state.view_once_tracking.deliveries.find(row => row.id === expired).status, 'unconfirmed');
        assert.equal(alice.core.viewOnceOutbox.letters.has(expired), false);
        const nextId=await alice.core.invoke('send_view_once',{contactId:1,ttlSecs:300,body:'discard on lock'});
        const remaining=alice.core.viewOnceOutbox.letters.get(nextId);
        const saved=storedState(senderName,alice);
        assert.equal(saved.view_once_tracking.deliveries.find(row=>row.id===nextId).status,'pending');
        assert.deepEqual(saved.messages,[]);
        alice.core.closeNow();
        assert.ok(remaining.key.every(b=>b===0) && remaining.parts.every(part=>part.every(b=>b===0)));
        closedCopy=BrowserEngine.open_password(senderName,pass,alice.snapshots.at(-1),new Uint8Array());
        const locked = new BrowserBackend(()=>{});
        locked.engine=closedCopy; locked.identity=alice.card; locked.state=JSON.parse(closedCopy.take_metadata());
        assert.equal(locked.viewOnceOutbox.due(Date.now()),undefined);
        assert.equal((await locked.invoke('list_view_once_deliveries')).find(row=>row.id===nextId).status,'pending');
        locked.closeNow(); closedCopy=null;

        const paused = endpoint('transaction lock');
        paused.state.contacts.push(contact(bob));
        let writing, finish;
        const entered = new Promise(resolve => { writing=resolve; });
        const release = new Promise(resolve => { finish=resolve; });
        paused.core.storage = { close() {}, async persist() { writing(); await release; } };
        const pending = paused.core.invoke('send_view_once',{contactId:1,body:'cancel an uncommitted send'});
        await entered;
        const held = [...paused.core.viewOnceOutbox.letters.values()][0];
        assert.equal(held.committed,false);
        assert.equal(paused.core.viewOnceOutbox.due(Date.now()),undefined);
        paused.core.closeNow(); finish();
        await assert.rejects(pending);
        assert.ok(held.key.every(b=>b===0) && held.parts.every(part=>part.every(b=>b===0)));

      } finally {
        if (closedCopy) { closedCopy.close();closedCopy.free(); }
        alice.core.closeNow(); bob.core.closeNow(); bob.relay.close(); alice.relay.close();
        await Promise.allSettled([...tasks]);
      }
    });
    const sender = BrowserEngine.create('second browser', pass);
    const senderCard = JSON.parse(sender.public_info());
    restored.contacts.push({ ...senderCard, id: 2, request: 'none', trust: 1 });
    const frames = [];
    const fail = () => assert.fail('view-once used an ordinary mailbox operation');
    const deposits = [];
    const receiver = new BrowserRelay(fresh, { close() {} }, browser, async (from, blob) => {
      assert.equal(from, senderCard.sign_pk); deposits.push(blob.slice());
    }, fail, () => {}, (from, req) => backend.receiveViewOnceOffer(from, req));
    let serving;
    const outgoing = new BrowserRelay(sender, { async connect(address) {
      assert.equal(address, browser.i2p_address);
      const [client, server] = streamPair(bytes => frames.push(JSON.parse(decode_client_frame(bytes))));
      serving = receiver.serve(server).catch(error => { if (String(error) !== 'Error: stream closed') throw error; });
      return client;
    }, close() {} }, senderCard, fail, fail, () => {});
    try {
      const text = 'x'.repeat(16 * 1024);
      const id = await outgoing.sendViewOnce(browser.i2p_address, browser.sign_pk, browser.dh_pk, new TextEncoder().encode(text), Date.now() + 300_000, () => true);
      assert.deepEqual(frames.map(frame => Object.keys(frame)[0]), ['AuthFramedV1', 'ViewOnceOfferV1', ...Array(5).fill('ViewOncePartV1')]);
      assert.equal(await backend.invoke('open_view_once', { id }), text);
      assert.deepEqual(deposits, []);
      frames.length = 0;
      const cipher = new Uint8Array(2 * 256 * 1024 + 25);
      for (let i = 0; i < cipher.length; i++) cipher[i] = i & 255;
      await outgoing.deposit(browser.i2p_address, browser.sign_pk, cipher);
      assert.deepEqual(deposits, [cipher]);
      assert.deepEqual(frames.filter(frame => frame.SendPartV1).map(frame => [frame.SendPartV1.index, frame.SendPartV1.total]),
        [[0,3], [1,3], [2,3]]);
      assert.ok(!frames.some(frame => frame.Send));
      frames.length = 0;
      await assert.rejects(outgoing.deposit(browser.i2p_address, peer.sign_pk, cipher), /ownership/);
      assert.deepEqual(frames.map(frame => Object.keys(frame)[0]), ['AuthFramedV1']);
      const challenge = crypto.getRandomValues(new Uint8Array(32));
      const nonce = crypto.getRandomValues(new Uint8Array(32));
      const proof = fresh.terminal_proof(senderCard.sign_pk, challenge, nonce);
      assert.ok(BrowserEngine.verify_terminal_proof(browser.sign_pk, browser.i2p_address, senderCard.sign_pk, challenge, nonce, proof));
      assert.equal(BrowserEngine.verify_terminal_proof(browser.sign_pk, senderCard.i2p_address, senderCard.sign_pk, challenge, nonce, proof), false);
      nonce[0] ^= 1;
      assert.equal(BrowserEngine.verify_terminal_proof(browser.sign_pk, browser.i2p_address, senderCard.sign_pk, challenge, nonce, proof), false);
    } finally {
      outgoing.close(); receiver.close(); await serving; sender.close(); sender.free();
    }
  } finally {
    backend?.closeNow();
    native.stdin.end();
    await rm(dir, { recursive: true, force: true });
  }
});
