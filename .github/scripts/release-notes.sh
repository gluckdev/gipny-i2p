#!/bin/bash
# Write the GitHub release page for a gipny-i2p release to stdout.
#
#   release-notes.sh <tag> <assets-dir> <owner/repo> [<git-ref>]
#
# <git-ref> is where the changelog ends: the tag itself on a tag build, HEAD on
# a dry run (where the tag does not exist yet). The changelog starts at the
# previous tag reachable from it.
#
# Strict about assets on purpose. Every platform row must match exactly one
# file, and every file must belong to a row: a release missing its Windows
# installer, or carrying a stray build by-product, fails here instead of going
# out looking finished. Optional hand-written highlights for a version live in
# docs/releases/<version>.md and go above everything else.
set -euo pipefail

TAG="$1"
DIR="$2"
REPO="$3"
REF="${4:-$TAG}"
VERSION="${TAG#v}"
BASE="https://github.com/$REPO/releases/download/$TAG"

# platform | what it is | exact file name
ROWS=(
  "Windows 10/11 · x64|установщик|gipny-i2p_${VERSION}_x64-setup.exe"
  "macOS 15+ · Apple Silicon|образ диска|gipny-i2p_${VERSION}_aarch64.dmg"
  "macOS 15+ · Intel|образ диска|gipny-i2p_${VERSION}_x64.dmg"
  "Linux · x86_64|AppImage, любой дистрибутив|gipny-i2p_${VERSION}_amd64.AppImage"
  "Linux · x86_64|.deb для Debian / Ubuntu|gipny-i2p_${VERSION}_amd64.deb"
  "Linux · ARM64|AppImage, любой дистрибутив|gipny-i2p_${VERSION}_aarch64.AppImage"
  "Linux · ARM64|.deb для Debian / Ubuntu / Raspberry Pi OS|gipny-i2p_${VERSION}_arm64.deb"
  "Android 7+ · 64-bit|APK|gipny-i2p_${VERSION}_android-arm64.apk"
  "Android 7+ · 32-bit|APK для старых телефонов|gipny-i2p_${VERSION}_android-armv7.apk"
  "Релей · Linux x86_64|сервер, tar.gz|gipny-relay_${VERSION}_linux-amd64.tar.gz"
  "Релей · Linux ARM64|сервер, tar.gz|gipny-relay_${VERSION}_linux-arm64.tar.gz"
  "Агент · Linux x86_64|консольный демон, tar.gz|gipny-agent_${VERSION}_linux-amd64.tar.gz"
  "Агент · Linux ARM64|консольный демон, tar.gz|gipny-agent_${VERSION}_linux-arm64.tar.gz"
)

human() { awk -v b="$1" 'BEGIN { split("Б КБ МБ ГБ", u, " "); i = 1; while (b >= 1024 && i < 4) { b /= 1024; i++ } if (i == 1) printf "%d %s", b, u[i]; else printf "%.1f %s", b, u[i] }'; }

claimed=()
table=""
for row in "${ROWS[@]}"; do
  IFS='|' read -r platform kind name <<<"$row"
  if [ ! -f "$DIR/$name" ]; then
    echo "release-notes: missing asset for \"$platform — $kind\": $name" >&2
    exit 1
  fi
  size="$(stat -c %s "$DIR/$name")"
  table+="| $platform | [$name]($BASE/$name) | $kind | $(human "$size") |"$'\n'
  claimed+=("$name")
done

for f in "$DIR"/*; do
  name="$(basename "$f")"
  [ "$name" = "SHA256SUMS.txt" ] && continue
  if ! printf '%s\n' "${claimed[@]}" | grep -qxF "$name"; then
    echo "release-notes: asset not in the download table: $name" >&2
    exit 1
  fi
done
[ -f "$DIR/SHA256SUMS.txt" ] || { echo "release-notes: SHA256SUMS.txt missing" >&2; exit 1; }

# Changelog: conventional-commit subjects since the previous tag, sorted into
# what users notice and what they do not.
PREV="$(git describe --tags --abbrev=0 "$REF^" 2>/dev/null || true)"
range="${PREV:+$PREV..}$REF"

new="" fixes="" other=""
n_other=0
while IFS=$'\t' read -r sha subject; do
  [ -n "$sha" ] || continue
  type="$(sed -nE 's/^([a-zA-Z]+)(\([^)]*\))?!?: .*/\1/p' <<<"$subject" | tr 'A-Z' 'a-z')"
  text="$(sed -E 's/^[a-zA-Z]+(\([^)]*\))?!?: //' <<<"$subject")"
  text="$(tr '[:lower:]' '[:upper:]' <<<"${text:0:1}")${text:1}"
  line="- $text ([\`$sha\`](https://github.com/$REPO/commit/$sha))"$'\n'
  case "$type" in
    feat|ui)       new+="$line" ;;
    fix|security)  fixes+="$line" ;;
    *)             other+="$line"; n_other=$((n_other + 1)) ;;
  esac
done < <(git log --no-merges --format='%h%x09%s' "$range")

highlights="docs/releases/$VERSION.md"

{
  echo "Анонимный мессенджер поверх i2p со сквозным шифрованием. Без номера телефона и почты; релей для доставки может поднять любой."
  echo
  if [ -f "$highlights" ]; then
    cat "$highlights"
    echo
  fi

  echo "## Скачать"
  echo
  echo "| Платформа | Файл | Что это | Размер |"
  echo "|---|---|---|---|"
  printf '%s' "$table"
  echo
  echo "Не знаете, какой APK? Берите **64-bit** — он подходит почти любому телефону, а на 32-битном просто не установится."
  echo
  echo "## Установка"
  echo
  echo "Сборки не подписаны платными сертификатами Microsoft и Apple, поэтому система предупредит при первом запуске. Это ожидаемо; проверить, что файл не подменён, можно по контрольным суммам ниже."
  echo
  echo "- **Windows** — SmartScreen покажет «Система Windows защитила ваш компьютер»: *Подробнее* → *Выполнить в любом случае*."
  echo "- **macOS** — перетащите приложение в «Программы». При первом запуске macOS его заблокирует: откройте *Системные настройки → Конфиденциальность и безопасность* и разрешите открытие. Или в терминале: \`xattr -dr com.apple.quarantine /Applications/gipny-i2p.app\`"
  echo "- **Linux** — AppImage: \`chmod +x gipny-i2p_*.AppImage\` и запустить. deb: \`sudo apt install ./gipny-i2p_*.deb\`"
  echo "- **Android** — разрешите установку из этого источника, когда телефон спросит. Релизы подписаны одним постоянным ключом, поэтому следующие версии ставятся поверх установленной."
  echo "- **Релей** — архив для своего сервера: \`gipny-relay\`, \`i2pd\` и systemd-юниты. Как развернуть — [в README](https://github.com/$REPO#свой-релей-нужно-поднять-до-полноценной-работы)."
  echo "- **Агент** — автономный демон \`gipny-agent\` для удалённого исполнения консольных команд мастером через E2E-канал I2P."
  echo
  echo "## Проверка файлов"
  echo
  echo "\`SHA256SUMS.txt\` содержит SHA-256 каждого файла. В папке со скачанным:"
  echo
  echo '```'
  echo "sha256sum -c SHA256SUMS.txt --ignore-missing"
  echo '```'
  echo
  echo "## Изменения"
  echo
  if [ -n "$new" ]; then echo "### Новое"; echo; printf '%s' "$new"; echo; fi
  if [ -n "$fixes" ]; then echo "### Исправления"; echo; printf '%s' "$fixes"; echo; fi
  if [ -n "$other" ]; then
    echo "<details><summary>Сборка, CI, документация и прочее ($n_other)</summary>"
    echo
    printf '%s' "$other"
    echo
    echo "</details>"
    echo
  fi
  if [ -n "$PREV" ]; then
    echo "**Все изменения**: https://github.com/$REPO/compare/$PREV...$TAG"
  fi
}
