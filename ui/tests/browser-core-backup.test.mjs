// Build browser-router/pkg with wasm-pack first. Exercise the browser commands
// through the production client, module worker, real WASM, Web Locks and OPFS.
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { createServer } from 'vite';
import { chromium } from 'playwright';

test('encrypted backup restores profile identity, messages and files with fresh credentials', async t => {
  const cacheDir = await mkdtemp(join(tmpdir(), 'gipny-backup-'));
  t.after(() => rm(cacheDir, { recursive: true, force: true }));
  const server = await createServer({ configFile: false, cacheDir, logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, fs: { allow: [resolve('..')] } },
    plugins: [{ name: 'offline-test', configureServer(server) {
      server.middlewares.use('/blank', (_, res) => { res.setHeader('Content-Type', 'text/html'); res.end('<html></html>'); });
      server.middlewares.use('/browser-config.json', (_, res) => { res.writeHead(404); res.end(); });
    } }],
  });
  await server.listen(); t.after(() => server.close());
  const browser = await chromium.launch({ executablePath: process.env.GIPNY_CHROMIUM || '/usr/bin/google-chrome' });
  t.after(() => browser.close());
  const page = await browser.newPage();
  await page.goto(server.resolvedUrls.local[0] + 'blank');
  await page.evaluate(async () => {
    const { BrowserCoreClient } = await import('/src/browser-core/client.ts');
    window.client = new BrowserCoreClient();
    window.rpc = (cmd, args) => client.invoke(cmd, args);
    const { registerBrowserFile } = await import('/src/features/files/browser-files.ts');
    window.register = registerBrowserFile;
  });
  const rpc = (cmd, args) => page.evaluate(([cmd, args]) => rpc(cmd, args), [cmd, args]);
  await rpc('vault_create', { profile: 'original', pass: 'source credential', displayName: 'Archive example' });
  const identity = await rpc('my_card');
  const id = await rpc('add_contact', { signPk: '01'.repeat(32), dhPk: '02'.repeat(32),
    i2pAddress: 'a'.repeat(52) + '.b32.i2p', name: 'Example contact' });
  const group = await rpc('create_group', { name: 'Example group', memberContactIds: [id] });
  await rpc('set_ui_data', { key: 'chat_prefs', json: JSON.stringify({ clearOnLock: false }) });
  await rpc('set_attachment_privacy', { enabled: false });
  const row = await page.evaluate(async id => {
    const data = new Uint8Array(250000);
    data.set(new TextEncoder().encode('attachment content visible only after decryption'), 196600);
    return rpc('send_message', { contactId: id, body: 'private backup text', browserFiles: [
      new File([data], 'private-file.dat'), new File([], 'empty.dat'),
    ] });
  }, id);
  await rpc('pin_contact_message', { contactId: id, messageId: row });
  const origFiles = await rpc('list_attachments', { messageId: row });
  await rpc('enroll_passkey', { credentialId: [9, 4], rpId: 'localhost', salt: Array(32).fill(4), prf: Array(32).fill(3) });
  assert.equal((await rpc('passkey_entries', { profile: 'original' })).length, 1);
  const snapshot = await page.evaluate(async () => {
    window.backup = await rpc('export_identity', { passphrase: 'archive credential' });
    const bytes = new Uint8Array(await backup.arrayBuffer());
    const text = new TextDecoder().decode(bytes);
    return { size: bytes.length, magic: text.slice(0, 8), leaks: ['source credential', 'archive credential',
      'private backup text', 'private-file.dat', 'Example contact', 'attachment content visible'].filter(value => text.includes(value)) };
  });
  assert.equal(snapshot.magic, 'GIPNYBA1'); assert.ok(snapshot.size > 250000); assert.deepEqual(snapshot.leaks, []);
  await assert.rejects(rpc('export_identity', { passphrase: 'short' }), /too short/);
  await page.evaluate(async () => {
    const { configureBrowserBackend } = await import('/src/app/api.ts');
    const { setLang } = await import('/src/features/localization/i18n.ts');
    const { Store } = await import('/src/app/state.ts');
    const { App } = await import('/src/app/app.ts');
    const { SettingsModal } = await import('/src/features/settings/settings.ts');
    configureBrowserBackend(client); setLang('en');
    const store = new Store();
    const app = new App(store);
    document.body.append(app.el);
    app.openModal(close => new SettingsModal(store, app, close).el);
  });
  await page.getByLabel('backup passphrase (at least 8 characters)').fill('archive credential');
  const [archive] = await Promise.all([
    page.waitForEvent('download'), page.getByRole('button', { name: 'Export backup', exact: true }).click(),
  ]);
  const archivePath = join(cacheDir, archive.suggestedFilename());
  await archive.saveAs(archivePath);
  await page.getByRole('dialog').press('Escape');
  await rpc('vault_lock');

  const restore = (profile, mode = 'good', password = 'archive credential') => page.evaluate(async ([profile, mode, password]) => {
    let file = backup;
    if (mode === 'corrupt') {
      const last = new Uint8Array(await file.slice(-1).arrayBuffer());
      last[0] ^= 1;
      file = new Blob([file.slice(0, -1), last]);
    }
    if (mode === 'truncated') file = file.slice(0, -30);
    if (mode === 'trailing') file = new Blob([file, Uint8Array.of(0)]);
    if (mode === 'native') file = new Blob(['GIPNYBAK']);
    await rpc('import_identity_to_profile', { profile, backupPass: password, vaultPass: 'replacement credential',
      ...(mode === 'blob' ? { backupFile: file } : { backupPath: register(new File([file], 'backup.bin')) }) });
  }, [profile, mode, password]);
  await assert.rejects(restore('restored', 'good', 'wrong credential'), /wrong credentials/);
  await assert.rejects(restore('original', 'blob'), /already exists/);
  for (const mode of ['corrupt', 'truncated', 'trailing', 'native']) {
    await assert.rejects(restore('restored', mode), /authentication failed|incomplete browser backup|Expected a gipny browser backup/);
    assert.deepEqual(await rpc('list_profiles'), ['original']);
  }
  // State a worker crash can leave after it flushed both slots but before
  // publication. Opening must fail; a retry reclaims only this staging tree.
  await page.evaluate(async () => {
    const root = await (await navigator.storage.getDirectory()).getDirectoryHandle('gipny-browser-v1');
    const bytes = new TextEncoder().encode('gipny/browser/profile/v1\0restored');
    const id = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(b => b.toString(16).padStart(2, '0')).join('');
    const staging = await root.getDirectoryHandle(id, { create: true });
    await staging.getFileHandle('restore.pending', { create: true });
    const vault = await (await staging.getFileHandle('vault-a.bin', { create: true })).createWritable();
    await vault.write('interrupted snapshot'); await vault.close();
    const label = await (await staging.getFileHandle('profile.json', { create: true })).createWritable();
    await label.write(JSON.stringify({ name: 'restored' })); await label.close();
  });
  assert.deepEqual(await rpc('list_profiles'), ['original']);
  await assert.rejects(rpc('vault_unlock', { profile: 'restored', pass: 'replacement credential' }), /restore is incomplete/);
  const [picker] = await Promise.all([
    page.waitForEvent('filechooser'), page.getByRole('button', { name: 'Import backup', exact: true }).click(),
  ]);
  await picker.setFiles(archivePath);
  const form = page.getByRole('dialog');
  assert.ok((await form.innerText()).includes(archive.suggestedFilename()));
  await form.getByPlaceholder('new profile name').fill('restored');
  await form.getByPlaceholder('new vault passphrase', { exact: false }).fill('replacement credential');
  await form.getByPlaceholder('backup passphrase', { exact: true }).fill('archive credential');
  await form.getByRole('button', { name: 'Import', exact: true }).click();
  await page.locator('#unlock-password').waitFor();
  assert.deepEqual(await rpc('list_profiles'), ['original', 'restored']);
  assert.deepEqual(await rpc('passkey_entries', { profile: 'restored' }), []);
  for (const pass of ['archive credential', 'source credential']) await assert.rejects(rpc('vault_unlock', { profile: 'restored', pass }), /wrong credentials/);
  await rpc('vault_unlock', { profile: 'restored', pass: 'replacement credential' });
  assert.deepEqual(await rpc('my_card'), identity);
  assert.equal((await rpc('list_groups'))[0].id, group);
  assert.equal((await rpc('get_contact', { id })).name, 'Example contact');
  assert.equal((await rpc('list_pinned_contact', { contactId: id }))[0].body, 'private backup text');
  assert.deepEqual(await rpc('list_attachments', { messageId: row }), origFiles);
  const bytes = Buffer.from(await rpc('load_attachment', { attachmentId: origFiles[0].id }), 'base64');
  assert.equal(bytes.length, 250000);
  assert.ok(bytes.subarray(196600).toString().startsWith('attachment content visible only after decryption'));
  assert.equal(await rpc('load_attachment', { attachmentId: origFiles[1].id }), '');
  const [download] = await Promise.all([page.waitForEvent('download'), rpc('export_identity', { destPath: 'archives/roundtrip.bin', passphrase: 're-exported archive' })]);
  assert.equal(download.suggestedFilename(), 'roundtrip.bin');
  assert.equal(await download.failure(), null);
  await rpc('vault_lock');
  await rpc('vault_unlock', { profile: 'original', pass: 'source credential' });
  assert.deepEqual(await rpc('my_card'), identity);
  assert.equal((await rpc('passkey_entries', { profile: 'original' })).length, 1);
  await rpc('vault_lock');
});
