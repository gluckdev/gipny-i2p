# Голосовые сообщения и звонки

## Пользование

Микрофон возле поля ввода открывает запись. Разрешите доступ к микрофону. Индикатор показывает входной уровень, таймер считает записанное время. На паузе микрофон выключен. Нажмите «Остановить», прослушайте запись и выберите «Отправить» или «Отмена». Запись автоматически останавливается через три минуты звука. У собеседника карточка показывает длительность. Кнопка воспроизведения открывает проигрыватель.

В шапке личного чата с принятым контактом нажмите трубку. Собеседник увидит имя звонящего и кнопку «Ответить». Затем появится проверка I2P-соединения. При разговоре видны длительность, выключатель микрофона и завершение звонка. Закрытие окна звонка, смена чата или блокировка профиля освобождают микрофон. Входящие вызовы принимаются при открытом, разблокированном приложении. Для браузерного микрофона нужны secure context, Web Audio и AudioWorklet.

## Транспорт и измерения

Запись сохраняется для отправки как моно PCM WAV, 16 кГц, 16 бит, максимум 5 760 044 байта. Имя содержит случайный идентификатор. До отправки черновик и предпросмотр находятся в памяти. Отправленную запись обрабатывают обычные правила вложений, доставки и истории чата. Получатель слышит голос автора без искажения.

Звонок использует сессию X3DH/Double Ratchet контакта и его I2P-релей. Приглашение, согласие, пробы, звук, подтверждения и завершение проходят этим же путём. Формат control-письма версии 1 содержит нулевой origin, установленное в false поле typing и префикс `gipny-call/1 `. Это позволяет действующим GUI-клиентам старого протокола обработать его как служебное уведомление. Вызов использует релей без fallback в DHT. Релей может временно хранить зашифрованный конверт. Принимающий клиент отбрасывает устаревшие кадры. Синхронизация часов между устройствами влияет на проверку свежести.

До отправки аудио каждая сторона измеряет время ответа на одну пробу и две параллельные пробы размером с аудиокадр. Стартовое окно — одна или две неподтверждённые передачи. Каждые 16 своевременных подтверждений окно может вырасти до четырёх. Долгий RTT или потеря уменьшают окно. Кодек G.711 μ-law работает в моно при 8 кГц. Клиент объединяет 250–750 мс звука в пакет по сглаженному RTT и доступному окну. При перегрузке старый звук вытесняется, поэтому на медленном маршруте возможны пропуски. На native используются доступные соединения из пула релея. В браузере каждая отправка идёт через JS-клиент и WASM-маршрутизатор. У звонка нет отдельного соединения WebRTC с собеседником или внешним SFU.

Проверка с двумя настоящими Chromium, двумя WASM-профилями, OPFS и локальным тестовым релеем выполняет вызов, согласие, обмен звуком в обе стороны, mute, hangup, запись и побайтовую сверку вложения. Команда из папки `ui` — `GIPNY_CHROMIUM=/path/to/chrome node tests/voice.test.mjs`. Живой сетевой тест в `browser-router/tests/run_messenger_ci.py` использует два независимых I2P-маршрутизатора. Для коммита `f1066e5` в обоих отчётах `browser-messenger-evidence` подтверждены двусторонние микрофонные кадры и ACK через I2P, отдельно с WSS и WebRTC между браузером и мостом. Оба сценария затем дошли до восстановления после перезапуска и завершились по таймауту офлайн-доставки. [GitHub Actions, run 44282718672](https://github.com/gluckdev/gipny-i2p/actions/runs/44282718672). Для оценки разборчивости речи и задержки нужен разговор людей на целевых устройствах.

## English

Use the microphone in the composer to record, pause, review and send a voice message. Clips use the existing encrypted attachment path. Tap the handset in an accepted contact’s chat to call. The recipient answers before their microphone starts. Both clients probe the I2P route, then adjust outstanding frames and packet duration using acknowledgements. The mute and hang-up controls are in the call dialog. Leaving that dialog or locking the profile stops capture. The app must be open and unlocked to ring. Audio quality depends on the route, and no public calling service is required beyond working Gipny I2P connectivity.

The recorder and player interaction draws on [MattermostVoiceClips](https://github.com/gluckdev/MattermostVoiceClips/tree/733f7ec). The call controls were compared with [the requested Calls fork](https://github.com/gluckdev/mattermost-plugin-calls), [upstream Mattermost Calls](https://github.com/mattermost/mattermost-plugin-calls) and [Element Call](https://github.com/element-hq/element-call). Gipny implements its own encrypted I2P media transport.
