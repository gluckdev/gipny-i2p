# gipny (i2p)

Анонимный мессенджер со сквозным шифрованием для десктопа и мобильных. Без телефона, без email, без логина. Личность — это пара ключей ed25519/x25519 плюс i2p‑адрес (destination), и больше ничего.

> **Про форк:** это **i2p‑форк** gipny. Транспорт переведён с Tor (Arti) на **i2p** (через go‑i2p и SAMv3). Крипта, хранилище, протокол релея и UI не менялись. Полный список изменений и текущий статус — в **[MIGRATION-i2p.md](MIGRATION-i2p.md)**.

- **Транспорт:** i2p через встроенный чисто‑Go‑роутер **go‑i2p** (SAMv3), из Rust общаемся крейтом `yosemite`. Роутер поставляется внутри приложения — ставить отдельно ничего не нужно.
- **Крипта:** первичный обмен X3DH + Double Ratchet (те же примитивы, что у Signal), AEAD XChaCha20‑Poly1305, Ed25519 + X25519.
- **Хранилище на диске:** SQLCipher (AES‑256) с KDF Argon2id. Пароль под принуждением (duress: стереть или подсунуть decoy), авто‑стирание при переборе, харднинг процесса (без core dump, mlock).
- **Минимизация метаданных:** sealed sender (`from = [0u8; 32]` в проводе — релей не видит отправителя), паддинг полезной нагрузки по фиксированным «корзинам» (размер не выдаёт текст/вложение/медиа).
- **Доставка:** один центральный «слепой» релей поверх i2p — никогда не P2P напрямую, но офлайн‑доставка работает (релей держит зашифрованные блобы, пока адресат не в сети).
- **Bot SDK:** полноценный Rust‑крейт для ботов: текст, файлы (мультивложения), inline‑кнопки — всё по тому же зашифрованному каналу.

---

## Как устроен транспорт

Приложение содержит небольшой бинарь `gipny-i2p-router` — обёртку вокруг встроенного SAM‑моста go‑i2p. При запуске приложение поднимает его дочерним процессом, ждёт готовности SAMv3 на `127.0.0.1:7656` и дальше автоматически гоняет весь трафик через i2p. Устанавливать вручную ничего не надо.

Цепочка: `ты → i2p‑туннели → релей gipny → destination получателя`. Первый запуск дольше последующих (i2p делает reseed и строит туннели, это 1–3 минуты). Системный VPN до запуска добавляет ещё один слой между твоим реальным IP и входом в i2p.

> Прежняя опция внешнего SOCKS5‑прокси удалена: в i2p трафик до локального роутера идёт по loopback, а маршрутизацией между узлами занимается сам роутер.

```
приложение (Rust) ──SAMv3 127.0.0.1:7656──▶ gipny-i2p-router (Go)
                                              ├─ встроенный роутер go-i2p
                                              └─ SAMv3‑мост
```

---

## Готовые сборки

Если просто хочешь пользоваться — забери артефакт под свою платформу со страницы **[Releases](../../releases)**, компилировать ничего не нужно.

### Linux

```bash
# AppImage (без установки, просто запуск)
chmod +x gipny-*.AppImage
./gipny-*.AppImage

# .deb (Debian / Ubuntu / Parrot / Mint / Kali)
sudo apt install ./gipny_*_amd64.deb

# .tar.gz (любой glibc‑дистрибутив)
tar -xzf gipny-*-linux-amd64.tar.gz
cd gipny-*-linux-amd64/
./gipny
```

### Windows

Два артефакта на релиз:
- `gipny-*-windows-x64-setup.exe` — установщик NSIS (рекомендуется).
- `gipny-*-windows-x64.zip` — портативный zip, распаковать и запустить `gipny.exe`.

Установщик кладёт gipny в `C:\Program Files\gipny\` и создаёт ярлык в меню «Пуск». Для портативного zip права администратора не нужны.

### Android — экспериментальная поддержка

На Android go‑i2p встроен непосредственно в процесс приложения через JNI:
Gradle собирает `libgipnyi2p.so` с Android NDK, а foreground service запускает
SAMv3 на `127.0.0.1:7656`. Release workflow собирает APK для `arm64-v8a`.
Поддержка пока считается экспериментальной до проверки на расширенной матрице
реальных устройств.

---

## Структура проекта

```
core/        Клиент на Tauri 2 (Rust + TS UI), десктоп и android
libcore/     Общий код: крипта, сессии, транспорт, БД, роутер
i2p-router/  Встроенный go‑i2p SAM‑роутер (Go), собирается в один бинарь
bot-sdk/     Фреймворк для ботов
ui/          UI на TypeScript (ванильный, без фреймворка)
.github/     GitHub Actions: build.yml (проверка) и release.yml (релиз)
```

Локальных сборочных скриптов нет — **все сборки только на GitHub Actions**.

Каталог `core/relay/` — крейт релей‑сервера. Он специально вне основного workspace и нужен только если поднимаешь свой релей.

---

## Сборка из исходников

Цели: **Linux** (AppImage / .deb / .tar.gz), **Windows** (установщик NSIS / портативный zip), **Android arm64** (экспериментальный APK). macOS пока не поддерживается.

Нужны два тулчейна:
- **Rust** (stable) — сам мессенджер. Ставится через [rustup](https://rustup.rs/).
- **Go** (1.23+) — встроенный i2p‑роутер (`i2p-router/`).

### Встроенный i2p‑роутер (Go)

```bash
cd i2p-router
CGO_ENABLED=0 go build -o gipny-i2p-router .   # чистый статический бинарь, кросс‑компилится под любую ОС
# положи его туда, где приложение его найдёт: рядом с бинарём gipny,
# в ресурсах Tauri, или укажи путь через переменную GIPNY_I2P_BIN
```

Проверка, что роутер жив:
```bash
./gipny-i2p-router --sam-listen 127.0.0.1:7656 --data ./router-data
# в другом окне:
printf 'HELLO VERSION MIN=3.0 MAX=3.3\n' | nc 127.0.0.1 7656   # → RESULT=OK
```

### Нативная dev‑сборка под Linux

Системные зависимости (имена для Debian/Ubuntu; в других дистрибутивах — аналоги):

```bash
sudo apt install -y build-essential pkg-config curl ca-certificates git \
    libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav gstreamer1.0-pulseaudio gstreamer1.0-alsa \
    libasound2-dev nodejs npm
```

Затем:

```bash
# роутер собираем прямо в core/resources — оттуда его берёт и tauri‑bundler,
# и dev‑запуск (иначе tauri‑build ругнётся, что resources‑glob пуст)
mkdir -p core/resources
cd i2p-router && CGO_ENABLED=0 go build -o ../core/resources/gipny-i2p-router . && cd ..
cd ui && npm install && npm run build && cd ..
cd core && GIPNY_I2P_BIN=$PWD/resources/gipny-i2p-router cargo run   # dev‑запуск
```

Окно Tauri откроется, как только соберётся бинарь. Первый старт i2p — 1–3 минуты (reseed + туннели).

### Все сборки — только на GitHub Actions

Локального сборщика нет; всё делается в CI:
- **`.github/workflows/build.yml`** — проверка компиляции на каждый push/PR: матрица go‑роутера (linux/windows) + Rust‑workspace + typecheck UI.
- **`.github/workflows/release.yml`** — полный релиз по тегу `v*`: собирает go‑роутер, кладёт его в ресурсы, собирает Linux (AppImage/deb) и Windows (NSIS) с зашитым роутером, публикует GitHub Release. Есть отдельная джоба **desktop xbox** (см. ниже).
- **`.github/workflows/codeql.yml`** — security‑скан (rust / js‑ts / actions).

Android APK подписывается постоянным release‑ключом. Для release workflow нужны
GitHub Actions secrets `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`,
`ANDROID_KEY_ALIAS` и `ANDROID_KEY_PASSWORD`.

Запуск релиза: `git tag v0.3.0 && git push origin v0.3.0` (или Actions → release → Run workflow).

---

## Вариант оформления «Xbox»

Помимо классического вида собирается **отдельное приложение** с оформлением в стиле Xbox — тот же бэкенд и те же функции, только внешний вид. Классика при этом не меняется и остаётся сборкой по умолчанию.

- Тема — это чистый CSS‑слой поверх существующего UI (`ui/src/themes/xbox.css`), подключается только при сборке варианта (`VITE_THEME=xbox`) через плагин Vite. Переключателя внутри приложения нет.
- У варианта свой app‑id и имя (`core/tauri.xbox.conf.json`), так что это отдельное приложение.
- В CI его собирает джоба **desktop xbox** (Linux AppImage).

Собрать локально:
```bash
cd ui && VITE_THEME=xbox npm run build && cd ..
cd core && cargo tauri build --config tauri.xbox.conf.json --bundles appimage
```

---

## Свой релей (нужно поднять до полноценной работы)

По умолчанию `DEFAULT_RELAY` пустой — клиент стартует и получает свой i2p‑адрес, но слать некуда, пока не указан релей. Разверни свой:

1. На сервере запусти go‑i2p‑роутер (юнит `core/relay/gipny-i2p-router.service`) — он поднимает SAMv3 на `127.0.0.1:7656`.
2. Запусти `gipny-relay` (юнит `core/relay/gipny-relay.service`, `GIPNY_RELAY_DATA=/var/lib/gipny-relay`). При первом старте он печатает `I2P DESTINATION: <base64>`.
3. Впиши это значение в `libcore/src/relay.rs::DEFAULT_RELAY` и пересобери. (Аналогично — сервер обновлений в `libcore/src/update.rs::DEFAULT_UPDATE_ONION`.)

Оба systemd‑юнита уже в `core/relay/` (релей зависит от юнита роутера через `Requires=`/`After=`). Пока адрес релея не зашит (или не задан в настройках), клиент по дизайну молчит.

> `DEFAULT_RELAY` пустой не из‑за проблем с генерацией ключей: destination от go‑i2p проверен — 391 байт, корректный KeyCertificate (Ed25519 + X25519), «повтор» в base64 это штатный i2p‑паддинг коротких ключей. Пусто просто потому, что боевой релей ещё не развёрнут; правильный адрес печатает сам `gipny-relay` при первом запуске.

---

## Bot SDK — быстрый старт

```rust
use gipny_bot::{Bot, BotTarget};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bot::builder()
        .data_dir("./bot-data")
        .display_name("stats-bot")
        .on_command("export", |ctx, _arg| async move {
            let today = generate_today_report().await?;
            ctx.send_attachments_with_buttons(
                "stats ready",
                vec![("today.csv".into(), today)],
                vec![vec![("refresh".into(), "export".into())]],
            ).await?;
            Ok(())
        })
        .build()?
        .run().await
}
```

Полный справочник: [bot-sdk/docs.md](bot-sdk/docs.md).

---

## Модель безопасности за 60 секунд

| Слой              | Что защищает                                                                 | Что всё ещё утекает                                                |
| ----------------- | --------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| Личность          | ed25519 + x25519 — ни телефона, ни email, ни логина; по ID не деанонить     | Если сам публично раскроешь свой i2p‑адрес — это на тебе          |
| Сеть              | i2p (входящие/исходящие туннели, чесночная маршрутизация)                    | i2p уязвим к статистическим атакам на уровне глобального наблюдателя |
| Сервер (релей)    | sealed sender — релей не видит отправителя; только `{получатель, блоб}`      | Один канонический релей = единая точка отказа доставки (не крипты)  |
| Контент           | X3DH + Double Ratchet + XChaCha20‑Poly1305 — полная forward secrecy         | Компрометация устройства с разблокированным хранилищем вскрывает текущую сессию |
| На диске          | SQLCipher (AES‑256) + Argon2id + duress‑пароль; **i2p‑ключ тоже в хранилище**, не открытым файлом; в памяти ключ в `Zeroizing` | Извлечение RAM методом cold‑boot теоретически возможно            |
| Размеры (метадата)| Паддинг по фиксированным корзинам (256 Б → 16 МиБ)                           | Выбор корзины всё же разбивает трафик на классы размеров           |

### Честные оговорки

- **go‑i2p — ранняя стадия** («probably not safe yet»); его streaming — прототип. Это именно тот слой, на котором держатся наши соединения → возможны стуоллы/переподключения. При нестабильности дизайн router‑agnostic: тот же SAMv3, можно подменить go‑i2p на i2pd без изменений клиента.
- Маленькая аудитория → меньше стороннего ревью, чем у Signal.
- Независимого крипто‑аудита пока нет.
- Один канонический релей; мульти‑релей — в планах.
- Нет мультиустройства: одна личность = один активный клиент. Для миграции — экспорт/импорт, затем закрыть старый клиент.
- Первое i2p‑соединение медленнее, чем в Tor (reseed + построение туннелей).

Экран **«Безопасность и анонимность»** внутри приложения содержит полный разбор.

---

## Где лежит профиль / данные

```
Linux:    $XDG_DATA_HOME/gipny/profiles/<имя>/      (по умолчанию: ~/.local/share/gipny/...)
Windows:  %APPDATA%/gipny/profiles/<имя>/
Android:  /data/user/0/app.gipny/gipny/profiles/<имя>/
```

В каждом профиле — база SQLCipher, состояние i2p‑роутера (`i2p/`), i2p‑идентичность (внутри зашифрованной БД, не открытым файлом) и блобы `attachments/`. Стереть профиль — удалить его каталог; duress/auto‑wipe затирает его рекурсивно с перезаписью.
