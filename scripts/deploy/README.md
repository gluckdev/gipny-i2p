# Размещение / Deployment

## Русский

Шаблоны отдельного серверного/локального `gipny-web` удалены по решению владельца
2026-11-06. Веб-клиент — статическая WASM/PWA-сборка; профили и криптография
находятся в браузере. Для сети нужен отдельный байтовый Rust-мост WSS/WebRTC.

Используйте [руководство веб-клиента](../../docs/browser-messenger.md) и
[руководство моста](../../browser-router/forwarder/README.md). Pages workflow
готовит WASM в `/app/browser.html`. Наличие workflow не означает развёрнутый мост.

## English

The separate server/local `gipny-web` templates were removed by owner decision
on 2026-11-06. The web client is a static WASM/PWA build with browser-owned
profiles and cryptography. Networking uses a separate Rust WSS/WebRTC byte bridge.
Follow the [browser guide](../../docs/browser-messenger.md) and
[bridge guide](../../browser-router/forwarder/README.md). Pages builds WASM at
`/app/browser.html`; a workflow is not a deployed transport service.
