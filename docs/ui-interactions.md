# UI interactions and contact import

The native and browser clients share these controls and the existing light/dark
palette. `ui/src/styles/interactions.css` owns the interaction effects separately
from the older layout stylesheet.

## Effects and profile card

- Buttons, list entries and fields transition their existing colors, borders and
  shadows over 160 ms. Keyboard focus keeps a visible outline; no geometry is
  animated by this effect layer.
- Modal backdrops use a fixed 2 px blur, including narrow phone layouts. The modal
  content and its action buttons stay sharp. Unsupported filters retain the
  existing dim backdrop.
- Reduced motion, reduced transparency or a slow-update display disable these
  transitions and the modal blur. Local optional `saveData`, at most two reported
  CPU threads, or at most 2 GiB reported device memory also select this fallback.
  Missing hints are normal. No battery API, telemetry or persistent device
  classification is used; this does not detect every OS power-saving mode.
- The QR icon beside the unlocked profile's name opens the existing identity
  card, using the same button and keyboard action. Locked profile selection does
  not expose an identity card.

## Adding contacts

Pasting a complete `gipny:v1:`, `gipny:v2:` or `gipny:v3:` card, or successfully
decoding it from the camera/image picker, validates and adds it automatically.
Addresses, keys, relay, UTF-8 name bounds and the v3 checksum are checked. Parsing
does **not** authenticate the person: new contacts retain the backend's unverified
TOFU state. Fingerprint verification remains a separate user action.

Ordinary typing only fills the fields; the Add button submits them. This matters
for legacy cards whose optional name can still be unfinished when the preceding
URI is already valid. Arbitrary text, partial cards, HTTP wrappers and transfer
links do not trigger an import or an external fetch.

The modal has one operation in flight. A shared in-memory guard also prevents two
modals for the same profile/key from submitting concurrently. Before adding, the
UI re-reads the vault's contacts. An existing signing key is not upserted, so its
saved trust, address and relay remain unchanged; a different identity DH key is
reported as a conflict. This is not an atomic lock across independent windows:
the backend remains responsible for admission and identity pinning.

Closing, locking or switching profiles invalidates pending checks and late scan
results. An already-dispatched backend operation can finish, but its old modal
does not refresh or show success in a new profile. Errors keep the form open and
release the guard for a retry. Camera tracks are stopped when the modal closes.

## Evidence and limits

- `node tests/contacts.test.mjs`: complete/invalid cards, actual input event path,
  manual submission, camera callback boundary, duplicate/conflicting/own identity,
  in-flight guards, close/lock/profile-switch races and retry; optional effect hints.
- `node tests/interactions.test.mjs`: real computed styles and QR/modal actions in
  both themes at 360×780, including reduced-motion/resource fallback. Local
  Chromium and Playwright WebKit passed. Select an installed engine with
  `GIPNY_UI_ENGINE=webkit` or `firefox`; Chromium is the default.
- `npm run test:layout` includes the existing size/language/keyboard matrix and
  this interaction smoke. `npm test` includes contact regressions.

Playwright WebKit is not physical Safari or native WKWebView evidence. Firefox
has not been exercised by this change's local run. Existing browser WASM/OPFS,
Cromite policy and platform coverage limits remain separate from these UI tests.
