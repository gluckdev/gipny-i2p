# Удаление сообщений / Message deletion

Состояние исходников: **2026-11-06**, изменения ещё требуют GitHub CI.
[Результаты прежних SHA](archive/message-deletion-2026-10-28.md) не проверяют эту версию.

## Русский

В личном диалоге любой участник может удалить своё сообщение или сообщение
собеседника. Текст и вложения удаляются локально сразу; запрос второй стороне
сохраняется до её аутентифицированного подтверждения. Доставка запроса релею
не означает удаления на устройстве собеседника.

На месте содержимого остаётся небольшая отметка: время исходного сообщения,
кто удалил («вами» или «собеседником») и время операции; дата показывается,
только если это не сегодня. Последовательные удаления одним участником
объединяются независимо от того, чьи это были сообщения: количество, период
исходных сообщений и время/период удаления. Живое сообщение, пропуск номера или
уже подтверждённая отметка разделяют группы. У инициатора к отметке добавляется
«ждём «ОК» собеседника»; «ОК» подтверждает группу частями по 500.

Зелёная кнопка **«ОК»** слева доступна другому участнику. Её нажатие сохраняется
локально и отправляется инициатору удаления. У инициатора отметка исчезает
после получения этого подтверждения. Автоматический сетевой ACK, открытие
чата, блокировка, очистка истории и перезапуск не заменяют нажатия «ОК».
При одновременном удалении обоими каждый подтверждает операцию другого.

Отметки не увеличивают unread и не создают уведомления. Удалённое содержимое
нельзя редактировать, пересылать или открыть как вложение. Само подтверждение
скрывает только отметку: защита от повторного появления содержимого остаётся.

### Протокол и хранение

Reference — публичный ключ автора, исходный `origin_msg_id` и `sent_at`.
Локальные номера `dm_seq` помогают группировать историю, но не являются общим
сетевым порядком. Controls E2E-аутентифицированы и относятся только к одному DM:

| `WireDm.kind` | Значение |
| --- | --- |
| 0 | Идентификатор обычного содержимого или file chunk |
| 1 | Удаление; время операции находится в envelope `sent_at` и не меняется при повторе |
| 2 | Приложение применило удаление; отметку не скрывает |
| 3 | Другой участник явно нажал «ОК» |
| 4 | Получение «ОК»; снимает очередь повторов kind 3 |

В native `dm_delete_outbox` и `dm_notice_outbox` — независимые сохраняемые
очереди. `dm_deletion_notices` хранит состояния обеих сторон, включая случай
одновременного удаления. Body, file keys и имена в ней не копируются.
Ручное подтверждение всей группы записывается одной транзакцией; чужой
contact/message ID отклоняет её целиком. Повтор control не меняет исходное
время и не возвращает скрытую отметку. Tombstones сохраняются после «ОК».

`Core` и `SessionManager` используют одну DB-реализацию. Браузер сохраняет
такие же обязательства внутри зашифрованного OPFS-профиля перед сетевыми
действиями; deposit не удаляет kind 1/3 из очереди. Уже подготовленные
шифротексты не отбрасываются с нарушением цепочки ratchet. Native backup
содержит metadata sidecar `dm_state_v2`, включая отметки и очередь «ОК».
Поддержка прежних данных и миграции не являются требованием этого изменения.

Удаление файлов использует общую транзакционную границу с приёмом chunk;
неудачный unlink остаётся работой для повторной очистки. Это не физическое
стирание SSD и не отзыв скриншотов, экспортированных файлов, OS notifications,
старых backups или данных модифицированного клиента. Групповое удаление
не расширено этим DM-протоколом.

### Проверка

Для CI добавлены native DB и browser regressions: автоматический ACK не
скрывает отметку; перезапуск; потеря receipt; повтор; удаление получателем;
чужой контакт; откат неполной группы; пять последовательных удалений и
разделители групп. Фактические результаты новых тестов пока не получены.

## English

Either participant may delete a direct message. Local text and attachments
are removed immediately; the authenticated remote deletion request remains
pending until acknowledged. Relay deposit does not prove device deletion.

A compact notice shows the original message time, who deleted it ("you" or
"your contact") and the deletion time, with the date only when it is not today.
Adjacent deletions by one participant form one notice whoever wrote the
messages, with a count and message/deletion time ranges. Live messages, sequence
gaps and dismissed notices separate groups. The initiator's notice adds "waiting
for your contact's OK"; OK acknowledges a group in batches of 500.

Only the other participant sees the green **OK** action. Pressing it persists
an acknowledgement and queues its delivery; the initiator dismisses the notice
when that acknowledgement arrives. Automatic receipts, history clearing,
reopening and restarts never substitute for this action. Simultaneous deletions
require each participant to acknowledge the other operation. Tombstones remain
after dismissal and still reject replayed text, edits and attachments.

`WireDm` kinds 1/2 mean delete/applied; kinds 3/4 mean explicit OK/receipt.
The reference includes the author key, original message ID and timestamp,
scoped to the authenticated contact. Deletion time remains stable on retries.
Native Core/SessionManager share transactional DB handling and independent
persistent outboxes. The browser persists equivalent obligations in its sealed
OPFS profile before network effects. New native backups carry `dm_state_v2`.
Legacy data compatibility is outside this change's requirements.

Notices create no unread counts or notifications. Deletion cannot revoke copies
outside cooperating applications or guarantee physical disk erasure. This is a
DM contract, not a new group-deletion protocol. CI regressions have been added
for manual/automatic acknowledgement, replay, restart, recipient deletion,
contact scoping, batch rollback and grouping. New CI results are still pending.
