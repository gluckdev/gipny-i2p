import type { GroupMember } from '../../app/api';
import { isValidI2pAddress } from '../../features/contacts/contact-card';
import { hex, unhex, type BrowserState, type PublicIdentity, type WireGroup } from '../profiles/model';

/** Merge introductions without allowing stale rosters to unpin identities or
 * remove later members. Called inside the backend's receive transaction. */
export function ingestGroup(state: BrowserState, identity: PublicIdentity, wire: WireGroup, peer: string): string {
  if (wire.id.length !== 32 || wire.name.length > 128 || wire.members.length > 64 || wire.members.length < 2) throw new Error('Invalid group');
  const id = hex(wire.id);
  const members: GroupMember[] = wire.members.map(member => {
    if (member.sign_pk.length !== 32 || member.dh_pk.length !== 32 || !isValidI2pAddress(member.onion) || member.name.length > 96) throw new Error('Invalid group member');
    return { sign_pk: hex(member.sign_pk), dh_pk: hex(member.dh_pk), onion: member.onion, name: member.name, is_self: hex(member.sign_pk) === identity.sign_pk };
  });
  for (const member of members) {
    const pinned = member.is_self ? identity : state.contacts.find(value => value.sign_pk === member.sign_pk);
    if (pinned && pinned.dh_pk !== member.dh_pk) throw new Error('Group member identity changed');
  }
  if (new Set(members.map(member => member.sign_pk)).size !== members.length || !members.some(member => member.is_self) || !members.some(member => member.sign_pk === peer)) throw new Error('Group sender is not a member');
  let group = state.groups.find(value => value.id === id);
  if (group && !group.members.some(member => member.sign_pk === peer)) throw new Error('Uninvited group sender');
  if (!group) { group = { id, name: wire.name, members, created_at: Date.now(), pinned_at: null, last_message_at: Date.now() }; state.groups.push(group); }
  else for (const member of members) {
    const known = group.members.find(value => value.sign_pk === member.sign_pk) || state.contacts.find(value => value.sign_pk === member.sign_pk);
    if (known && known.dh_pk !== member.dh_pk) throw new Error('Group member identity changed');
    if (!group.members.some(value => value.sign_pk === member.sign_pk)) { if (group.members.length >= 64) throw new Error('Group member limit reached'); group.members.push(member); }
  }
  // An established member can introduce a new member. Existing identity keys
  // stay pinned, and a stale message cannot remove later additions.
  return id;
}

export function groupWire(state: BrowserState, id: string): WireGroup {
  const group = state.groups.find(value => value.id === id); if (!group) throw new Error('Group not found');
  return { id: unhex(id), name: group.name, members: group.members.map(member => ({ sign_pk: unhex(member.sign_pk), dh_pk: unhex(member.dh_pk), onion: member.onion, name: member.name })) };
}
