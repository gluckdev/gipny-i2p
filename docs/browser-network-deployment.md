# Восстановление сетевого сервиса публичного browser-клиента

Проверено read-only 2026-11-16. Ни сервер, ни DNS, ни GitHub variables не
изменялись; новые облачные ресурсы не создавались. Основная архитектура —
собственный Emissary I2P router в браузере. WSS/WebRTC forwarder передаёт
транспортные bytes к публичным I2P peers; он не заменяет router или messenger
core серверным процессом. Ниже разделены уже доказанная работа этой архитектуры,
состояние проверенного Pages URL и подготовленный вариант размещения byte bridge.

## Публикация 18 ноября 2026 года

GitHub Pages опубликовал веб-клиент из `main` на коммите `896ec23`.
Проверка сборки в Chrome открыла экран профиля повторно после отключения
сети. Публичный `app/browser-config.json` при проверке 18 ноября вернул
HTTP 404. Доставка сообщений через этот адрес требует конфигурации моста.
[Запуск деплоя](https://github.com/gluckdev/gipny-i2p/actions/runs/44265938257) ·
[Порядок публикации](../scripts/deploy/README.md).

## Состояние Pages при проверке 16 ноября

- `https://gluckdev.github.io/gipny-i2p/app/browser.html` возвращает HTTP 200,
  соседний `browser-config.json` — HTTP 404.
- Последний успешный [Pages run 42515393363](https://github.com/gluckdev/gipny-i2p/actions/runs/42515393363)
  построен из `e124b2fa50f9ae37bbada8173e7e2361984b5b25`. В
  [build log](https://github.com/gluckdev/gipny-i2p/actions/runs/42515393363/job/128837176879)
  в `2026-11-08T19:32:13.994Z` записано:
  `No public network configuration supplied; the WASM client opens with offline storage only.`
- Repository variable `GIPNY_BROWSER_CONFIG_JSON` отсутствует; у environment
  `github-pages` variables отсутствуют. Проверены только имена secrets, их
  значения не читались: binding для bridge/host/deployment не найден.
- [`pages.yml`](../.github/workflows/pages.yml) читает эту repository variable.
  [`prepare-browser-config.py`](../scripts/prepare-browser-config.py) при пустом
  значении намеренно успешно завершает сборку без конфигурационного файла.
- Такое состояние уже описано в [`browser-messenger.md`](browser-messenger.md#hosting-status)
  и [`handoff-brief.md`](handoff-brief.md).

Этот deployment не содержит endpoint, с которым мог бы начать I2P; новый startup
loader в нём ещё не опубликован. Точный URL, открытый на realme/Chrome, пока
не подтверждён. Если это указанный Pages URL, отсутствие network config
объясняет сообщение о недоступном сервисе; для другого URL этот вывод требует
отдельной проверки. Изменить только сообщение ошибки недостаточно.
GitHub Pages предоставляет [статический хостинг](https://docs.github.com/en/pages/getting-started-with-github-pages/what-is-github-pages);
сам forwarder должен исполняться отдельно.

## Где browser-owned I2P уже работал

- В [CI run 41329839707](https://github.com/gluckdev/gipny-i2p/actions/runs/41329839707)
  на `fa60b02` две независимые браузерные Emissary instances прошли все семь
  acceptance assertions. Assets, config и snapshot обслуживались локально;
  harness поднимал временный byte forwarder. Результат зафиксирован в
  [`browser-validation-2026-11-01.md`](archive/browser-validation-2026-11-01.md).
  На том checkpoint публичное размещение и физический телефон не проверялись.
- 7 ноября [`WORK_NOTES.md`](../WORK_NOTES.md) фиксирует успешный Browser↔Honor:
  browser tunnels через 64 с, сообщение телефону через 32 с, ответ браузеру
  через 2 с. Браузер владел своим I2P router; Honor работал через native app.
  Воспроизводитель [`run_browser_phone.py`](../browser-router/tests/run_browser_phone.py)
  обслуживает `http://localhost:18770/browser.html`, snapshot/config на том же
  origin и `ws://127.0.0.1:18767/stream`. Локальные процессы принадлежат harness.
  Это реальный успешный сетевой тест, но не проверка публичного Pages endpoint.

История публикации объясняет различие между этими результатами:

| Изменение | Подтверждённый эффект |
| --- | --- |
| `7e99adc` → `fceeafc` → `318b2d5` → `426e2ec` → `36981ca`, 1 ноября | Добавлены WASM runtime, NTCP2 через byte forwarder, downloaded bootstrap, in-process client sessions и WASM Worker UI |
| [`e9a4721`](https://github.com/gluckdev/gipny-i2p/commit/e9a4721b3d98fb910836c4aab18c5d294e1e449e), 6 ноября | Pages переведён с `npm run build`/`ui/dist` на WASM + `build:browser`/`ui/browser-dist`; network config читается из необязательной repository variable |
| `6c77c5d`, 8 ноября | Уже существующее отсутствие config отображается как unconfigured/offline; исчезает ложное ожидание прогресса. Сетевой runtime или рабочий публичный endpoint этим commit не удалены |

Проверены build logs всех восьми успешных main Pages deployments с WASM
7–8 ноября. Каждый содержит `No public network configuration supplied`:

| Pages run | Source SHA | Время записи в build log, UTC |
| --- | --- | --- |
| [42328944743](https://github.com/gluckdev/gipny-i2p/actions/runs/42328944743) | `1f00d33` | 2026-11-07 02:26:29 |
| [42334765042](https://github.com/gluckdev/gipny-i2p/actions/runs/42334765042) | `1f00d33` | 2026-11-07 03:37:54 |
| [42485097051](https://github.com/gluckdev/gipny-i2p/actions/runs/42485097051) | `e911857` | 2026-11-08 13:29:43 |
| [42491327619](https://github.com/gluckdev/gipny-i2p/actions/runs/42491327619) | `535d74b` | 2026-11-08 14:46:41 |
| [42491373707](https://github.com/gluckdev/gipny-i2p/actions/runs/42491373707) | `da6a677` | 2026-11-08 14:53:06 |
| [42508233029](https://github.com/gluckdev/gipny-i2p/actions/runs/42508233029) | `44452fb` | 2026-11-08 18:07:34 |
| [42513530704](https://github.com/gluckdev/gipny-i2p/actions/runs/42513530704) | `e124b2f` | 2026-11-08 19:10:17 |
| [42515393363](https://github.com/gluckdev/gipny-i2p/actions/runs/42515393363) | `e124b2f` | 2026-11-08 19:32:13 |

В просмотренной истории всех refs нет конкретного публичного bridge config:
встречаются fixtures и примеры. Удаление ранее работавшей публичной variable
по этим данным не установлено. Доказан разрыв между успешным локальным/CI
browser-owned path и просмотренными Pages artifacts; это не опровержение
работавшего браузерного I2P и не доказательство необходимости новой VM.

## Проверка исторического кандидата для byte bridge

В проекте есть реализация `gipny-forwarder`, CI artifact и инструкции; действующий
публичный endpoint в проверенных источниках не найден. Узкая проверка локального проекта
GCP обнаружила историческую VM `gcp-free` в project `cloud-gcp-free`, однако
сводка содержит разные зоны/адреса и не описывает forwarder. Это лишь кандидат
для проверки существующих ресурсов; доступность, свободная ёмкость, ОС и
владелец порта 443 пока не установлены. Платная VM и Cloudflare Tunnel не входят
в предложенный вариант. Прежние локальные `gipny-web` templates были удалены
6 ноября; это другой механизм, а не скрытый работающий browser bridge.

Live read-only проверка SDK обнаружила активный gcloud login, но запрос
`compute instances list --project=cloud-gcp-free --filter=name=gcp-free` вернул
`The resource 'projects/cloud-gcp-free' was not found` и пустой список. Это не
позволяет отличить удалённый проект от недоступного текущей учётной записи и
не подтверждает существование VM. Старые IP не проверялись через SSH; облачные
настройки не изменялись. Исторический адрес нельзя считать действующим.
Этот результат не исключает существующий сервис по другому адресу и не
является основанием создавать новую VM.

## Минимальный пилот на существующем сервере

Это подготовленный вариант на случай, если после уточнения открытого URL и
существующего endpoint потребуется восстановить размещение byte bridge.
Он не требует переноса браузерного router на сервер. Начальный вариант
использует уже поддерживаемый WSS transport `/stream`.
Он не требует TURN/UDP provisioning. Это выбор для ограниченного пилота, а не
утверждение о максимальной скорости или эквивалентной наблюдаемости WSS/WebRTC.
NTCP2 остаётся сквозным между браузерным router и I2P peer; bridge видит IP
браузера, выбранные публичные router endpoints и timing/volume.

Для review такого размещения потребуются следующие сведения, без передачи
секретов в чат; это перечень зависимостей плана, а не запрос создать ресурсы:

1. Существующий Linux host и его актуальный способ администрирования (например,
   работающий SSH/IAP alias), согласованный для запуска bridge и обновления netDb.
2. Принадлежащий оператору hostname, направленный на этот хост, и возможность
   обслужить TLS443 без нарушения уже работающих сервисов.
3. Возможность разместить HTTPS snapshot на том же hostname, обновлять публичные
   данные и repository variable `GIPNY_BROWSER_CONFIG_JSON`.

Далее используются placeholders `network.example.invalid` и локальные пути;
они не являются работающими адресами. Capability генерируется на хосте как
32 случайных bytes в unpadded base64url (43 символа), хранится в файле с режимом 0600.
В публичном browser config будет её копия: это не пароль профиля и не защита от
клиента, которому уже известен токен. Ограничения сервера остаются обязательными.

Сборка берётся из одного проверенного source SHA: artifact
`browser-forwarder-linux` workflow `Browser router WASM`. Собранные для того же
pin инструменты `collect-netdb` и `prepare-netdb` готовят только public RouterInfos.
Нет `--allow-fixture-*`, укороченных application tunnels или отключённого padding.

```sh
/opt/gipny/gipny-forwarder \
  --listen 127.0.0.1:18767 \
  --host network.example.invalid \
  --origin https://gluckdev.github.io \
  --capability-file /etc/gipny/forwarder-capability \
  --candidates /srv/gipny/candidates.tsv \
  --max-connections 32 --session-seconds 600 --idle-seconds 120 \
  --max-bytes 67108864
```

Origin включает схему и hostname, **без `/gipny-i2p/app/`**. Listener только
loopback; TLS завершается локальным proxy. Отдельный непривилегированный service
user читает capability/candidates, не получает содержимое пользовательских профилей.
32 — текущий hard maximum процесса, не оценка достаточности для публичного
масштаба: один browser router может держать до 16 NTCP2 sessions.
Сам forwarder ограничивает global dial burst 32 с пополнением 1/с, каждый target
burst 4 и затем 1 dial/15 с, время/байты session и проверяет Host/Origin/capability,
подпись, network, свежесть и адрес/порт RouterInfo до соединения с целью.
Полные границы: [`forwarder README`](../browser-router/forwarder/README.md).

## TLS proxy и публичный snapshot

Ниже фрагмент nginx для существующего TLS certificate. `map` находится в
`http {}`, `server` подключается в обычный site config. До reload требуется
успешный `nginx -t`; actual certificate/domain paths подставляет оператор.
Upgrade/Connection headers передаются явно по
[официальной схеме nginx](https://nginx.org/en/docs/http/websocket.html).

```nginx
map $http_upgrade $gipny_connection_upgrade {
    default upgrade;
    '' close;
}

server {
    listen 443 ssl;
    server_name network.example.invalid;
    ssl_certificate /etc/letsencrypt/live/network.example.invalid/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/network.example.invalid/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    if ($host != network.example.invalid) { return 421; }
    access_log off;
    error_log /var/log/nginx/gipny-error.log warn;
    client_max_body_size 64k;
    client_header_timeout 10s;
    client_body_timeout 10s;

    location = /stream {
        proxy_pass http://127.0.0.1:18767;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header Origin $http_origin;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $gipny_connection_upgrade;
        proxy_set_header Sec-WebSocket-Protocol $http_sec_websocket_protocol;
        proxy_connect_timeout 5s;
        proxy_read_timeout 75s;
        proxy_send_timeout 75s;
        proxy_buffering off;
    }

    location /snapshots/ {
        root /srv/gipny/public;
        try_files $uri =404;
        autoindex off;
        add_header Access-Control-Allow-Origin https://gluckdev.github.io always;
        add_header X-Content-Type-Options nosniff always;
        default_type application/gzip;
    }
    location / { return 404; }
}
```

До публикации необходимо проверить, что общий nginx error log не сохраняет
auth headers/payloads и что proxy не отбрасывает WebSocket Ping. Forwarder
посылает Ping каждые 20 с; read timeout 75 с оставляет запас. Transport payloads,
capabilities, SDP и private profiles не входят в diagnostics artifacts.

Публичный snapshot и allowlist формируются в staging, например:

```sh
/opt/gipny/collect-netdb /srv/gipny/router-state
/opt/gipny/prepare-netdb /srv/gipny/router-state \
  /srv/gipny/staging/candidates.tsv 2048 /srv/gipny/staging/snapshot.tar.gz
```

Collector работает до 2500 routers или 360 с и отключает transit. Packer проверяет
RI и в snapshot-mode требует минимум 16 usable public peers, не добавляет local
fixture. Проверяется его manifest `snapshot.tar.gz.json`, включая SHA256 и pin.
После успеха snapshot копируется под неизменяемым именем
`/srv/gipny/public/snapshots/<sha256>.tar.gz`; candidates публикуется атомарной
заменой. Старый snapshot пока сохраняется для уже выданной конфигурации.

Forwarder перечитывает candidates каждые 5 мин, lease истекает не позднее 10 мин
после проверки либо 36 ч после публикации RI; ошибка reload не продлевает lease.
Для постоянной эксплуатации нужен работающий refresh job свежего netDb и
согласованное обновление snapshot/hash/config до устаревания peers. Единственная
ручная загрузка не создаёт постоянно работающий сервис. Scheduler ещё не
развёрнут: это обязательная часть provisioning, отдельная от публикации shell.

## Привязка Pages и проверка результата

Публичная repository variable должна содержать реальные согласованные значения:

```json
{
  "transport": "wss",
  "snapshot_url": "https://network.example.invalid/snapshots/<sha256>.tar.gz",
  "snapshot_sha256": "<64 hex characters from manifest>",
  "forwarder_url": "wss://network.example.invalid/stream",
  "capability": "<43 base64url characters matching the host file>"
}
```

Сначала validate посредством `scripts/prepare-browser-config.py` без печати
содержимого; затем установить repository variable и выполнить согласованный
Pages deployment. Сам по себе деплой новой UI/WASM сборки без variable сохранит
offline состояние. Текущий published SHA и новая application 3/3 реализация
должны быть записаны отдельно; успешный старый 2/2 CI не подтверждает новый baseline.

Критерий завершения — HTTP 200 и корректная схема `browser-config.json`, CORS
доступность snapshot с совпадающим digest, `READY targets > 0 rtc=off` у bridge,
фактическая application Destination readiness и двусторонний подтверждённый
обмен двух профилей. Проверить realme/Chrome в обычном режиме, затем lock/reopen,
восстановление с public cache и cold start на чистом тестовом профиле. Не удалять
реальный OPFS профиль телефона ради проверки. Wrong Origin/token, private target
и stale RI должны по-прежнему отвергаться; retry не должен скрывать ошибки.

Пока существующий host/domain/admin access не подтверждены, этот план остаётся
подготовкой. Наличие source code, зелёного Pages job или новой формулировки toast
не считается восстановленной сетевой доставкой.
