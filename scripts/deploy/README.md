# Старые шаблоны серверного gipny-web

`setup-gcp.sh`, `gipny-web.service` и `nginx-deusexxx.online.conf` относятся
к прежней схеме серверного gateway. Это исторические шаблоны, а не инструкция
развёртывания автономного браузерного мессенджера. Они предполагают нативный
бэкенд за nginx; упоминание домена не подтверждает наличие работающего сервера
или доступных сертификатов. Не применяйте их к новой PWA.

Актуальный локальный `gipny-web` описан в [core/README.md](../../core/README.md#gipny-web):
loopback, capability и явные Origin/Host проверки. Публичный reverse proxy из
старого шаблона не соответствует этой модели по умолчанию.

Автономный браузерный клиент использует `browser-messenger-pwa` из GitHub Actions,
HTTPS-статику, `browser-config.json`, проверенный снимок netDb и отдельный
байтовый WSS-форвардер. Конфигурация и команда его запуска приведены в
[docs/browser-messenger.md](../../docs/browser-messenger.md#hosting-preparation).
Профили, приватные ключи и криптография в этой схеме находятся в браузере.
