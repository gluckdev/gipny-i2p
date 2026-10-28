#!/usr/bin/env bash
# Install with: curl -fsSL .../install-agent.sh | bash -s -- --master '<card>'
set -euo pipefail
umask 077

# Also sourced by regressions; verification finishes before any install writes.
master_card_shape() {
  case $1 in
    gipny:v2:*) return 0 ;;
    gipny:v3:*) [[ $1 =~ ^gipny:v3:[A-Za-z0-9_-]{135,307}$ ]] ;;
    *) return 1 ;;
  esac
}
verify_staged_master() {
  master_card_shape "$2" || return 1
  case $2 in
    gipny:v3:*) "$1" --check-master-card "$2" >/dev/null 2>&1 ;;
  esac
}

verify_agent_archive() {
  local archive=$1 manifest=$2 asset_name=$3 expected actual
  [[ $(wc -c < "$manifest") -le 1048576 ]] || { echo 'Oversized checksum manifest' >&2; return 1; }
  expected=$(awk -v wanted="$asset_name" '
    { sub(/\r$/, ""); if ($0 ~ /^[[:space:]]*$/) next;
      digest=substr($0,1,64); separator=substr($0,65,2); name=substr($0,67);
      if (length(digest)!=64 || digest ~ /[^0-9a-fA-F]/ || (separator!="  " && separator!=" *") || name=="") { bad=1; next }
      sub(/^\.\//,"",name); if (name==wanted) { count++; expected=tolower(digest) }
    }
    END { if (bad || count!=1) exit 1; print expected }
  ' "$manifest") || { echo 'Missing, duplicate or malformed agent checksum' >&2; return 1; }
  if command -v sha256sum >/dev/null; then actual=$(sha256sum "$archive");
  elif command -v shasum >/dev/null; then actual=$(shasum -a 256 "$archive");
  else echo 'sha256sum or shasum is required' >&2; return 1; fi
  actual=${actual%% *}
  [[ $actual == "$expected" ]] || { echo 'Agent archive checksum mismatch' >&2; return 1; }
}
if [[ -n ${BASH_SOURCE[0]:-} && ${BASH_SOURCE[0]} != "$0" ]]; then return; fi

REPO=gluckdev/gipny-i2p
MASTER='' NAME='' DATA='' CWD='' RELAY='' TIMEOUT=120 VERSION=latest ACTION=install
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }
help() {
  cat <<'HELP'
GIPNY · установка удалённого агента

  bash install-agent.sh --master <карточка> [параметры]

  --master <card>   Карточка мастера (без параметра — запрос через /dev/tty)
  --name <name>     Имя агента (по умолчанию постоянное кошачье имя)
  --data <dir>      Данные: /var/lib/gipny-agent или ~/.local/share/gipny-agent
  --cwd <dir>       Рабочий каталог команд (по умолчанию домашний)
  --relay <relay>   Внешний релей (по умолчанию встроенный)
  --timeout <secs>  Таймаут команды, по умолчанию 120 секунд
  --version <ver>   GitHub release tag или latest
  --status          Статус службы и последние логи
  --uninstall       Остановить и удалить службу, бинарник, конфигурацию и данные
  -h, --help        Эта справка

Linux: systemd / systemd --user / OpenRC; иначе run-agent.sh + cron @reboot.
macOS: launchd. Нужны curl (или wget) и tar; зависимости дистрибутива
не устанавливаются автоматически. Без systemd/cron запуск после перезагрузки
нужно добавить вручную по напечатанной команде.
HELP
}
while (($#)); do
  case "$1" in
    --help|-h) help; exit 0 ;;
    --uninstall) ACTION=uninstall; shift ;;
    --status) ACTION=status; shift ;;
    --master|--name|--data|--cwd|--relay|--timeout|--version)
      (($# >= 2)) && [[ -n $2 ]] || fail "для $1 требуется значение"
      case "$1" in
        --master) MASTER=$2 ;; --name) NAME=$2 ;; --data) DATA=$2 ;;
        --cwd) CWD=$2 ;; --relay) RELAY=$2 ;; --timeout) TIMEOUT=$2 ;; --version) VERSION=$2 ;;
      esac
      shift 2 ;;
    *) fail "неизвестный параметр: $1 (см. --help)" ;;
  esac
done
[[ $TIMEOUT =~ ^[1-9][0-9]*$ ]] || fail '--timeout должен быть положительным целым числом'
case $(uname -s) in Linux) OS=linux ;; Darwin) OS=darwin ;; *) fail 'поддерживаются Linux и macOS' ;; esac
[[ ! -f /etc/openwrt_release ]] || fail 'для OpenWrt используйте install-agent-openwrt.sh и статическую musl-сборку'
case $(uname -m) in x86_64|amd64) ARCH=amd64 ;; arm64|aarch64) ARCH=arm64 ;; *) fail 'поддерживаются amd64 и arm64' ;; esac
if [[ $(id -u) == 0 ]]; then
  BIN=/usr/local/bin CONFIG=/etc/gipny-agent ENV=/etc/gipny-agent.env DEFAULT_DATA=/var/lib/gipny-agent
  UNIT=/etc/systemd/system/gipny-agent.service SYSTEMCTL=(systemctl)
else
  BIN=$HOME/.local/bin CONFIG=$HOME/.config/gipny-agent ENV=$HOME/.config/gipny-agent.env DEFAULT_DATA=$HOME/.local/share/gipny-agent
  UNIT=$HOME/.config/systemd/user/gipny-agent.service SYSTEMCTL=(systemctl --user)
fi
PLIST=$HOME/Library/LaunchAgents/com.gipny.agent.plist
RUN=$CONFIG/run-agent.sh
if [[ -z $DATA && -f $CONFIG/data-dir ]]; then IFS= read -r DATA < "$CONFIG/data-dir"; fi
DATA=${DATA:-$DEFAULT_DATA}
[[ $DATA == /* && $DATA != / && $DATA != "$HOME" && $DATA != /usr && $DATA != /var && $DATA != /etc && $DATA != /tmp ]] || fail '--data должен быть абсолютным путём к отдельному каталогу агента'
[[ $DATA != *$'\n'* ]] || fail 'перевод строки в пути данных не поддерживается'
[[ $DATA != "$BIN" && $DATA != "$CONFIG" && $DATA != "$(dirname "$CONFIG")" ]] || fail 'данные должны лежать отдельно от бинарника и конфигурации'
for value in "$MASTER" "$NAME" "$DATA" "$CWD" "$RELAY" "$HOME"; do
  [[ $value != *$'\n'* && $value != *$'\r'* ]] || fail 'переводы строк в параметрах не поддерживаются'
done
MODE=fallback
if [[ $OS == darwin ]]; then MODE=launchd
elif command -v systemctl >/dev/null && "${SYSTEMCTL[@]}" show-environment >/dev/null 2>&1; then MODE=systemd
elif [[ $(id -u) == 0 ]] && command -v rc-service >/dev/null && command -v rc-update >/dev/null; then MODE=openrc
fi
# Use the installed backend for status/removal even if its manager is currently down.
if [[ $ACTION != install && -f $CONFIG/backend ]]; then IFS= read -r MODE < "$CONFIG/backend"; fi
logs() {
  case $MODE in
    systemd) if [[ $(id -u) == 0 ]]; then printf 'journalctl -u gipny-agent -f'; else printf 'journalctl --user -u gipny-agent -f'; fi ;;
    *) printf 'tail -f %q' "$CONFIG/agent.log" ;;
  esac
}
stop() {
  case $MODE in
    systemd) "${SYSTEMCTL[@]}" disable --now gipny-agent >/dev/null 2>&1 || true ;;
    launchd) launchctl unload -w "$PLIST" 2>/dev/null || true ;;
    openrc) rc-service gipny-agent stop || true; rc-update del gipny-agent default || true ;;
    fallback)
      if [[ -f $CONFIG/agent.pid ]]; then
        pid=$(cat "$CONFIG/agent.pid")
        if [[ $pid =~ ^[0-9]+$ ]] && ps -p "$pid" -o args= | grep -F -- "$BIN/gipny-agent" >/dev/null; then
          kill "$pid"
          for ((i=0; i<40; i++)); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
          kill -0 "$pid" 2>/dev/null && fail 'агент ещё завершает работу; повторите позже'
        fi
      fi ;;
  esac
}
if [[ $ACTION == status ]]; then
  case $MODE in
    systemd) "${SYSTEMCTL[@]}" status gipny-agent --no-pager || true; "${SYSTEMCTL[@]/systemctl/journalctl}" -u gipny-agent -n 25 --no-pager || true ;;
    launchd) launchctl list com.gipny.agent || true ;;
    openrc) rc-service gipny-agent status || true ;;
    fallback)
      if [[ -f $CONFIG/agent.pid ]]; then ps -p "$(cat "$CONFIG/agent.pid")" -o pid,args || printf 'Агент не запущен\n'
      else printf 'Агент не запущен\n'; fi ;;
  esac
  [[ ! -f $CONFIG/agent.log ]] || tail -n 25 "$CONFIG/agent.log"
  printf '\nДанные: %s\nЛоги: ' "$DATA"; logs; printf '\n'; exit 0
fi
if [[ $ACTION == uninstall ]]; then
  # Only recursively remove a directory marked by this installer.
  [[ ! -e $DATA || -f $DATA/.gipny-agent-install ]] || fail "нет метки установщика в $DATA; данные не удалены"
  stop
  if command -v crontab >/dev/null && crontab -l >/dev/null 2>&1; then
    cron=$(crontab -l | sed '/# gipny-agent-autostart$/d'); printf '%s\n' "$cron" | crontab -
  fi
  rm -f "$UNIT" "$ENV" "$PLIST" "$BIN/gipny-agent" "$BIN/i2pd-netdb-seed.tar.gz"
  [[ $MODE != openrc ]] || rm -f /etc/init.d/gipny-agent
  rm -rf -- "$CONFIG" "$DATA"
  [[ $MODE != systemd ]] || "${SYSTEMCTL[@]}" daemon-reload
  printf 'GIPNY · агент удалён\n'; exit 0
fi
if [[ -z $MASTER ]]; then
  printf 'Ваша карточка GIPNY (агент первым свяжется с вами): ' > /dev/tty || fail 'передайте --master <card>'
  IFS= read -r MASTER < /dev/tty || fail 'передайте --master <card>'
fi
[[ $MASTER != *$'\n'* && $MASTER != *$'\r'* ]] || fail 'переводы строк в карточке не поддерживаются'
master_card_shape "$MASTER" || fail 'нужна карточка gipny:v3 (или v2) с релеем из настроек мастера'
if [[ -z $NAME && -f $CONFIG/agent-name ]]; then IFS= read -r NAME < "$CONFIG/agent-name"; fi
if [[ -z $NAME ]]; then
  cat_names=(Murka Barsik Matroskin Felix Luna Nala Simba Tigra Tom Bagheera)
  NAME=${cat_names[RANDOM % ${#cat_names[@]}]}
fi
[[ -z $CWD || -d $CWD ]] || fail '--cwd должен указывать на существующий каталог'
[[ -z $CWD ]] || CWD=$(cd -- "$CWD" && pwd -P)
fetch() {
  if command -v curl >/dev/null; then curl -fLsS --retry 3 "$1" -o "$2"
  elif command -v wget >/dev/null; then wget -q "$1" -O "$2"
  else fail 'установите curl или wget'; fi
}
TMP=$(mktemp -d); trap 'rm -rf -- "$TMP"' EXIT
if [[ $VERSION == latest ]]; then API="https://api.github.com/repos/$REPO/releases/latest"
else [[ $VERSION =~ ^[a-zA-Z0-9._-]+$ ]] || fail 'некорректный тег версии'; API="https://api.github.com/repos/$REPO/releases/tags/$VERSION"; fi
printf 'GIPNY · загрузка агента (%s/%s, %s)…\n' "$OS" "$ARCH" "$VERSION"
fetch "$API" "$TMP/release.json"
release_version=$(sed -n 's/^[[:space:]]*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' "$TMP/release.json")
[[ $release_version =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][a-zA-Z0-9.+-]+)?$ ]] || fail 'некорректная версия релиза'
asset_name="gipny-agent_${release_version}_${OS}-${ARCH}.tar.gz"
release_urls=$(sed -n 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$TMP/release.json")
URL=''; checksum_url=''; asset_count=0; manifest_count=0
while IFS= read -r candidate; do
  case $candidate in
    "https://github.com/$REPO/releases/download/"*)
      if [[ ${candidate##*/} == "$asset_name" ]]; then URL=$candidate; asset_count=$((asset_count+1)); fi
      if [[ ${candidate##*/} == SHA256SUMS.txt ]]; then checksum_url=$candidate; manifest_count=$((manifest_count+1)); fi ;;
  esac
done <<< "$release_urls"
[[ $asset_count == 1 && $manifest_count == 1 ]] || fail "в релизе нет единственного $asset_name и SHA256SUMS.txt"
fetch "$URL" "$TMP/agent.tar.gz"
fetch "$checksum_url" "$TMP/SHA256SUMS.txt"
verify_agent_archive "$TMP/agent.tar.gz" "$TMP/SHA256SUMS.txt" "$asset_name" || fail 'проверка архива не пройдена; установка не начата'
tar -tzf "$TMP/agent.tar.gz" > "$TMP/members"
# Release archives contain a single directory; reject paths escaping extraction.
if grep -E '(^/|(^|/)\.\.(/|$))' "$TMP/members" >/dev/null; then fail 'небезопасные пути в архиве'; fi
mkdir "$TMP/unpack"; tar -xzf "$TMP/agent.tar.gz" -C "$TMP/unpack"
EXE=$(find "$TMP/unpack" -type f -name gipny-agent | head -n 1)
SEED=$(find "$TMP/unpack" -type f -name i2pd-netdb-seed.tar.gz | head -n 1)
[[ -n $EXE && -n $SEED ]] || fail 'в архиве отсутствует агент или снимок сети'
chmod 0755 "$EXE"
verify_staged_master "$EXE" "$MASTER" || fail 'карточка повреждена или эта версия агента не поддерживает v3; нужны 0.4.19 или новее'
[[ ! -f $CONFIG/backend ]] || stop
mkdir -p "$BIN" "$CONFIG" "$DATA"; chmod 700 "$CONFIG" "$DATA"
install -m 0755 "$EXE" "$BIN/gipny-agent"; install -m 0644 "$SEED" "$BIN/i2pd-netdb-seed.tar.gz"
touch "$DATA/.gipny-agent-install"
printf '%s\n' "$DATA" > "$CONFIG/data-dir"; printf '%s\n' "$MODE" > "$CONFIG/backend"
printf '%s\n' "$NAME" > "$CONFIG/agent-name"
ARGS=(--data "$DATA" --master "$MASTER" --name "$NAME" --timeout "$TIMEOUT")
[[ -z $CWD ]] || ARGS+=(--cwd "$CWD")
[[ -z $RELAY ]] || ARGS+=(--relay "$RELAY")
{ printf '#!/usr/bin/env bash\nexec '; printf '%q ' "$BIN/gipny-agent" "${ARGS[@]}"; printf '\n'; } > "$RUN"
chmod 700 "$RUN"
# systemd EnvironmentFile quoting, including nested quotes used by $ARGS expansion.
arg_string=''
for arg in "${ARGS[@]}"; do
  arg=${arg//\\/\\\\}; arg=${arg//\"/\\\"}; arg_string+="\"$arg\" "
done
escaped=${arg_string//\\/\\\\}; escaped=${escaped//\"/\\\"}
printf 'GIPNY_AGENT_ARGS="%s"\n' "$escaped" > "$ENV"
case $MODE in
  systemd)
    mkdir -p "$(dirname "$UNIT")"
    env_path=${ENV//%/%%}; exe_path=${BIN//%/%%}
    env_path=${env_path//\\/\\\\}; env_path=${env_path//\"/\\\"}
    exe_path=${exe_path//\\/\\\\}; exe_path=${exe_path//\"/\\\"}
    target=multi-user.target; [[ $(id -u) == 0 ]] || target=default.target
    cat > "$UNIT" <<UNIT
[Unit]
Description=GIPNY headless agent
After=network-online.target
[Service]
Type=simple
EnvironmentFile="$env_path"
ExecStart="$exe_path/gipny-agent" \$GIPNY_AGENT_ARGS
Restart=on-failure
RestartSec=5
[Install]
WantedBy=$target
UNIT
    "${SYSTEMCTL[@]}" daemon-reload
    "${SYSTEMCTL[@]}" enable --now gipny-agent
    "${SYSTEMCTL[@]}" --no-pager status gipny-agent || true
    if [[ $(id -u) != 0 ]]; then printf 'Для работы без входа в систему: sudo loginctl enable-linger %q\n' "$(id -un)"; fi ;;
  launchd)
    xml() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
    mkdir -p "$(dirname "$PLIST")"
    cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.gipny.agent</string>
<key>ProgramArguments</key><array><string>/bin/bash</string><string>$(xml "$RUN")</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>ThrottleInterval</key><integer>5</integer>
<key>StandardOutPath</key><string>$(xml "$CONFIG/agent.log")</string>
<key>StandardErrorPath</key><string>$(xml "$CONFIG/agent.log")</string>
</dict></plist>
PLIST
    launchctl load -w "$PLIST" ;;
  openrc)
    mkdir -p /etc/init.d
    cat > /etc/init.d/gipny-agent <<'RC'
#!/sbin/openrc-run
name="GIPNY agent"
command="/etc/gipny-agent/run-agent.sh"
command_background=true
pidfile="/run/gipny-agent.pid"
output_log="/etc/gipny-agent/agent.log"
error_log="/etc/gipny-agent/agent.log"
depend() { need net; }
RC
    chmod 755 /etc/init.d/gipny-agent; rc-update add gipny-agent default; rc-service gipny-agent start ;;
  fallback)
    # Skip an already running process when invoked by cron or manually.
    START=$CONFIG/start-agent.sh
    cat > "$START" <<START
#!/usr/bin/env bash
set -e
if [[ -f $(printf '%q' "$CONFIG/agent.pid") ]]; then
  pid=\$(cat $(printf '%q' "$CONFIG/agent.pid"))
  if [[ \$pid =~ ^[0-9]+$ ]] && ps -p "\$pid" -o args= | grep -F -- $(printf '%q' "$BIN/gipny-agent") >/dev/null; then exit 0; fi
fi
nohup $(printf '%q' "$RUN") >> $(printf '%q' "$CONFIG/agent.log") 2>&1 < /dev/null &
printf '%s\\n' "\$!" > $(printf '%q' "$CONFIG/agent.pid")
START
    chmod 700 "$START"; "$START"
    if command -v crontab >/dev/null; then
      cron=$(crontab -l 2>/dev/null | sed '/# gipny-agent-autostart$/d') || true
      # Cron treats percent signs specially, even inside quotes.
      boot=$(printf '%q' "$START"); boot=${boot//%/\\%}
      printf '%s\n@reboot %s # gipny-agent-autostart\n' "$cron" "$boot" | crontab -
    else printf 'Автозапуск: добавьте %s в startup вашей системы.\n' "$START"; fi ;;
esac
printf '\n✓ GIPNY · агент установлен и запущен (%s)\n  Данные: %s\n  Логи: ' "$MODE" "$DATA"
logs; printf '\n  Агент появится в контактах мастера после подключения к i2p.\n'
