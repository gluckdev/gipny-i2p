import type { Message } from './api';

export type DeletionRun = { message: Message; members: Message[] };

/** Group only adjacent message positions, never across a live or dismissed row.
 *  A run is one participant's deletions (`can_ack`: the contact deleted, we
 *  may confirm); whose messages they were does not split it, so clearing a
 *  conversation leaves one notice, not one per change of speaker. */
export function deletionRuns(messages: readonly Message[]): DeletionRun[] {
  const rows: DeletionRun[] = [];
  let previous: Message | undefined;
  for (const message of messages) {
    const last = rows[rows.length - 1];
    if (message.deleted_at != null && message.deletion_notice === null) { previous = undefined; continue; }
    const notice = message.deletion_notice;
    const before = previous?.deletion_notice;
    const adjacent = previous?.dm_seq != null && message.dm_seq != null && message.dm_seq === previous.dm_seq + 1;
    if (message.deleted_at != null && previous?.deleted_at != null && notice && before && adjacent
      && notice.can_ack === before.can_ack && last) last.members.push(message);
    else rows.push({ message, members: [message] });
    previous = message;
  }
  return rows;
}
