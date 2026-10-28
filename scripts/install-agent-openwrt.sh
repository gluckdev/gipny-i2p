#!/bin/sh
# BusyBox ash installer. GitHub HTTPS is used only by this explicit installer;
# the installed agent's transport and updater continue to use I2P.
set -eu
umask 077

fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }
fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
  elif command -v uclient-fetch >/dev/null 2>&1; then uclient-fetch -q -O "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then wget -q -O "$2" "$1"
  else fail 'нужен curl, uclient-fetch или wget с проверкой HTTPS'; fi
}

manifest_digest() {
  # Parse the entire manifest. Malformed or duplicate rows fail even if a
  # preceding row matched; no optional/missing checksum path exists.
  awk -v asset="$2" '
    NF == 0 { next }
    {
      name=$2; sub(/^\*/, "", name)
      if (NF != 2 || length($1) != 64 || $1 !~ /^[0-9a-fA-F]+$/ ||
          name !~ /^[A-Za-z0-9_.+-]+$/ || name == "." || name == "..") { bad=1; next }
      if (seen[name]++) bad=1
      if (name == asset) digest=tolower($1)
    }
    END { if (bad || digest == "") exit 1; print digest }
  ' "$1"
}

validate_master() {
  # Match the headless card's structural rules without printing its contents.
  printf '%s\n' "$1" | awk -F: '
    function address(a, h) {
      if (a ~ /\.[bB]32\.[iI]2[pP]$/) {
        h=substr(a, 1, length(a)-8)
        return length(h)==52 && h ~ /^[a-zA-Z2-7]+$/
      }
      sub(/==?$/, "", a)
      return length(a)>=516 && a ~ /^[a-zA-Z0-9~-]+$/
    }
    /^gipny:v3:/ {
      payload=substr($0,10)
      if (length(payload)<135 || length(payload)>307 || payload !~ /^[A-Za-z0-9_-]+$/) exit 1
      next
    }
    { valid=($1=="gipny" && $2=="v2" && $3!="" &&
        length($4)==64 && $4 ~ /^[0-9a-fA-F]+$/ &&
        length($5)==64 && $5 ~ /^[0-9a-fA-F]+$/)
      if ($2=="v2") valid=valid && address($6)
      if (!valid) exit 1
    }
  '
}

help() {
  cat <<'HELP'
GIPNY · агент OpenWrt (BusyBox / procd)

  sh install-agent-openwrt.sh [--master <карточка>] [параметры]

  --master <card>   Карточка мастера; без параметра запрашивается через /dev/tty
  --name <name>     Явное имя; иначе стабильное кошачье имя выбирает агент
  --prefix <dir>    Отдельный каталог бинарника (по умолчанию /opt/gipny-agent)
  --data <dir>      Постоянные данные (существующая настройка или <prefix>/data)
  --version <tag>   Версия релиза или latest
  -h, --help        Справка

Нужны root, procd, uci, jsonfilter, tar, sha256sum и HTTPS-загрузчик с CA.
Для небольшого flash используйте постоянный USB/extroot, а не /tmp или /var.
Поддерживаются AArch64, ARMv7 little endian soft-float, x86_64 и i686/SSE2.
MIPS/MIPSel и ARMv6 soft-float пока не выпускаются. Данные при ошибке загрузки
или проверки не меняются. Старые ключи/база при обновлении не удаляются.
HELP
}

# Installer entry point (the functions above also have offline regression tests).
master='' name='' prefix='' data='' version=latest
while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help) help; exit 0 ;;
    --master|--name|--prefix|--data|--version)
      [ "$#" -ge 2 ] && [ -n "$2" ] || fail "для $1 требуется значение"
      case "$1" in
        --master) master=$2 ;; --name) name=$2 ;; --prefix) prefix=$2 ;;
        --data) data=$2 ;; --version) version=$2 ;;
      esac
      shift 2 ;;
    *) fail "неизвестный параметр: $1" ;;
  esac
done
[ -f /etc/openwrt_release ] || fail 'это установщик OpenWrt; для Linux/macOS используйте install-agent.sh'
[ "$(id -u)" = 0 ] || fail 'установщик procd нужно запускать от root'
for utility in jsonfilter sha256sum tar awk uci; do
  command -v "$utility" >/dev/null 2>&1 || fail "не найден $utility; установите его штатным менеджером пакетов OpenWrt"
done
[ -f /etc/rc.common ] || fail 'не найден procd /etc/rc.common'
if [ -z "$master" ]; then
  [ -r /dev/tty ] || fail 'передайте --master <карточка> для неинтерактивной установки'
  printf 'GIPNY: вставьте карточку мастера из приложения: ' > /dev/tty
  IFS= read -r master < /dev/tty || fail 'карточка мастера не введена'
fi
validate_master "$master" || fail 'некорректная карточка мастера'
cr=$(printf '\r')
for value in "$master" "$name" "$prefix" "$data" "$version"; do
  case "$value" in *"$cr"*|*'
'*) fail 'переводы строк в параметрах не поддерживаются' ;; esac
done
case "$version" in *[!a-zA-Z0-9._-]*|'') fail 'некорректный тег версии' ;; esac
if [ -z "$prefix" ]; then
  previous_binary=$(uci -q get gipny-agent.main.binary || true)
  if [ -n "$previous_binary" ]; then prefix=$(dirname "$previous_binary"); else prefix=/opt/gipny-agent; fi
fi
if [ -z "$data" ]; then data=$(uci -q get gipny-agent.main.data || true); fi
data=${data:-$prefix/data}
for directory in "$prefix" "$data"; do
  case "$directory" in /*) ;; *) fail 'prefix и data должны быть абсолютными путями' ;; esac
  case "$directory" in /|/etc|/usr|/bin|/sbin|/opt|/mnt|/tmp|/var|/root) fail 'выберите отдельный каталог агента' ;; esac
  case "$directory" in /tmp/*|/var/*|*/../*|*/..|*/./*) fail 'данные и бинарник должны быть в постоянном отдельном каталоге' ;; esac
done
[ "$prefix" != "$data" ] || fail 'каталог данных должен отличаться от каталога бинарника'
case "$(uname -m)" in
  aarch64|arm64) target=aarch64-unknown-linux-musl ;;
  armv7l|armv8l) target=armv7-unknown-linux-musleabi ;;
  x86_64|amd64) target=x86_64-unknown-linux-musl ;;
  i386|i486|i586|i686)
    grep -qw sse2 /proc/cpuinfo || fail '32-bit x86 требует SSE2 (Pentium 4 или новее)'
    target=i686-unknown-linux-musl ;;
  *) fail 'для этого CPU/ABI нет проверенного статического релиза; см. docs/openwrt.md' ;;
esac

mkdir -p "$prefix"
available=$(df -Pk "$prefix" | awk 'NR==2 {print $4}')
[ -n "$available" ] && [ "$available" -ge 65536 ] || fail 'для установки нужно минимум 64 MiB свободного места; выберите постоянный USB/extroot через --prefix'
stage=$(mktemp -d "$prefix/.install.XXXXXX")
trap 'rm -rf "$stage"' EXIT
trap 'exit 1' HUP INT TERM
repo=gluckdev/gipny-i2p
if [ "$version" = latest ]; then endpoint=latest; else endpoint="tags/$version"; fi
fetch "https://api.github.com/repos/$repo/releases/$endpoint" "$stage/release.json"
tag=$(jsonfilter -i "$stage/release.json" -e '@.tag_name')
case "$tag" in v[0-9]*) ;; *) fail 'не удалось определить тег релиза' ;; esac
case "$tag" in *[!a-zA-Z0-9._-]*) fail 'некорректный тег в ответе GitHub' ;; esac
[ "$version" = latest ] || [ "$tag" = "$version" ] || fail 'GitHub вернул другую версию'
package="gipny-agent_${tag#v}_${target}"
asset="$package.tar.gz"
jsonfilter -i "$stage/release.json" -e '@.assets[*].name' > "$stage/assets.txt"
[ "$(grep -xcF "$asset" "$stage/assets.txt" || true)" = 1 ] || fail 'нет единственного ассета для этой версии и ABI'
[ "$(grep -xcF SHA256SUMS.txt "$stage/assets.txt" || true)" = 1 ] || fail 'нет единственного SHA256SUMS.txt'
base="https://github.com/$repo/releases/download/$tag"
fetch "$base/SHA256SUMS.txt" "$stage/SHA256SUMS.txt"
digest=$(manifest_digest "$stage/SHA256SUMS.txt" "$asset") || fail 'SHA256 отсутствует, неоднозначен или manifest повреждён'
fetch "$base/$asset" "$stage/$asset"
printf '%s  %s\n' "$digest" "$stage/$asset" | sha256sum -c - >/dev/null || fail 'SHA256 архива не совпал'
tar -xzf "$stage/$asset" -C "$stage" "$package/gipny-agent" "$package/i2pd-netdb-seed.tar.gz"
binary="$stage/$package/gipny-agent"
[ -f "$binary" ] && [ ! -L "$binary" ] || fail 'в архиве нет обычного файла gipny-agent'
chmod 0755 "$binary"
status=0
"$binary" --help > "$stage/help.txt" 2>&1 || status=$?
{ [ "$status" = 0 ] || [ "$status" = 1 ]; } && grep -qF 'Usage: gipny-agent' "$stage/help.txt" || fail 'бинарник не запускается на этом CPU/ядре'
case "$master" in
  gipny:v3:*) "$binary" --check-master-card "$master" >/dev/null 2>&1 || fail 'карточка повреждена или версия агента не поддерживает v3; нужны 0.4.19 или новее' ;;
esac
[ ! -d "$prefix/gipny-agent" ] && [ ! -d "$data/master.card" ] || fail 'путь бинарника или master.card занят каталогом'

# Network, checksum, archive and executable checks have finished. Do not stop
# an existing service or modify its data before reaching this point.
if [ -x /etc/init.d/gipny-agent ]; then /etc/init.d/gipny-agent stop; fi
mv "$binary" "$prefix/gipny-agent"
mv "$stage/$package/i2pd-netdb-seed.tar.gz" "$prefix/i2pd-netdb-seed.tar.gz"
mkdir -p "$data"
chmod 0700 "$data"
master_file=$(mktemp "$data/.master.card.XXXXXX")
printf '%s\n' "$master" > "$master_file"
mv "$master_file" "$data/master.card"
cat > "$stage/service" <<'SERVICE'
#!/bin/sh /etc/rc.common
START=95
STOP=10
USE_PROCD=1
start_service() {
  config_load gipny-agent
  local binary data name
  config_get binary main binary
  config_get data main data
  config_get name main name
  [ -x "$binary" ] && [ -s "$data/master.card" ] || return 1
  procd_open_instance
  procd_set_param command "$binary" --data "$data"
  [ -z "$name" ] || procd_append_param command --name "$name"
  procd_set_param stdout 1
  procd_set_param stderr 1
  procd_set_param term_timeout 75
  # No automatic respawn: the master's explicit OFF must leave it stopped.
  procd_close_instance
}
SERVICE
chmod 0755 "$stage/service"
mv "$stage/service" /etc/init.d/gipny-agent
uci set gipny-agent.main=agent
uci set "gipny-agent.main.binary=$prefix/gipny-agent"
uci set "gipny-agent.main.data=$data"
if [ -n "$name" ]; then uci set "gipny-agent.main.name=$name"; fi
uci commit gipny-agent
/etc/init.d/gipny-agent enable
/etc/init.d/gipny-agent start
printf '%s\n' 'Агент установлен; procd принял запуск. Состояние: /etc/init.d/gipny-agent status'
printf '%s\n' 'Карточка появится в каталоге данных после запуска I2P. Операционные события: logread -e gipny-agent'
