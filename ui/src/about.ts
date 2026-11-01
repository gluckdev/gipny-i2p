import { t as tr } from './i18n';
import { usesBrowserCore } from './api';
import { h, type Child } from './view';
import { icon, networkArt } from './icons';

/** "О gipny и безопасности": what the app does with your messages, in plain
 * words. Every claim here must match the code — check it when behaviour
 * changes (delivery: libcore/src/session.rs, core/src/core.rs; relay:
 * libcore/src/relay_server.rs; vault: libcore/src/security.rs; updates:
 * libcore/src/update.rs). */
export class AboutModal {
  el: HTMLElement;

  constructor(close: () => void) {
    const card = (icon: string, title: string, ...body: Child[]): HTMLElement =>
      h('section', { class: 'about-card' },
        h('div', { class: 'about-card-icon' }, icon),
        h('div', { class: 'about-card-main' },
          h('h3', { class: 'about-card-title' }, title),
          ...body,
        ),
      );
    const p = (...text: Child[]): HTMLElement => h('p', { class: 'about-p' }, ...text);
    const steps = (...items: [string, string][]): HTMLElement =>
      h('ol', { class: 'about-steps' }, ...items.map(([t, d]) => h('li', null, h('b', null, t), ' ', d)));
    const list = (...items: Child[]): HTMLElement =>
      h('ul', { class: 'about-list' }, ...items.map((i) => h('li', null, i)));

    if (usesBrowserCore()) {
      this.el = h('div', { class: 'modal modal-wide about' },
        h('div', { class: 'modal-header' },
          h('div', { class: 'modal-title' }, tr('sidebar.about')),
          h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
        ),
        h('div', { class: 'modal-body' },
          card('🔑', tr('about.your_network_identity'),
            p(tr('about.creating_a_profile_generates_an_ed25519_signing_key')),
            p(tr('browser.about.contacts'))),
          card('✉️', tr('about.how_a_message_is_delivered'),
            p(tr('about.the_first_message_establishes_an_x3dh_session_double')),
            p(tr('browser.about.transport')), p(tr('browser.about.delivery'))),
          card('🛡️', tr('about.protection_on_your_device'),
            p(tr('browser.about.storage')), p(tr('browser.about.memory')),
            p(tr('browser.about.offline'))),
          card('⬆️', tr('about.updates'), p(tr('browser.about.updates'))),
          card('⚖️', tr('about.known_limitations'),
            p(tr('browser.about.limits')),
            p(tr('about.there_has_been_no_independent_code_or_cryptography')),
            p(tr('about.an_observer_with_visibility_over_the_whole_network'))),
        ),
        h('div', { class: 'modal-footer' },
          h('button', { class: 'btn', onClick: close }, tr('about.got_it'))),
      );
      return;
    }

    this.el = h('div', { class: 'modal modal-wide about' },
      h('div', { class: 'modal-header' },
        h('div', { class: 'modal-title' }, tr('sidebar.about')),
        h('button', { class: 'icon-btn', title: tr('common.close'), onClick: close }, icon('close')),
      ),
      h('div', { class: 'modal-body' },
        h('div', { class: 'about-hero' },
          networkArt(),
          h('div', { class: 'about-hero-title' }, tr('about.messaging_without_a_phone_number_email_or_login')),
          h('div', { class: 'about-hero-sub' },
            tr('about.your_identity_is_a_key_pair_created_on')),
          h('div', { class: 'about-chips' },
            ...[tr('about.end_to_end_encryption'), tr('about.i2p_network'), tr('about.no_registration'), tr('about.open_source')].map((t) => h('span', { class: 'about-chip' }, t)),
          ),
        ),

        card('🔑', tr('about.your_network_identity'),
          p(tr('about.creating_a_profile_generates_an_ed25519_signing_key')),
          p(tr('about.you_choose_your_display_name_it_travels_inside')),
        ),

        card('✉️', tr('about.how_a_message_is_delivered'),
          steps(
            [tr('about.encryption'), tr('about.the_first_message_establishes_an_x3dh_session_double')],
            [tr('about.size_padding'), tr('about.messages_are_padded_to_standard_sizes_between_256')],
            [tr('about.routing_through_i2p'), tr('about.the_embedded_i2pd_router_sends_messages_through_encrypted')],
            [tr('about.recipient_s_relay'), tr('about.each_client_has_a_relay_that_starts_with')],
            [tr('about.acknowledgement'), tr('about.the_recipient_collects_and_decrypts_the_message_then')],
          ),
        ),

        card('🤝', tr('about.new_contacts'),
          p(tr('about.adding_someone_s_card_sends_them_a_request')),
          p(tr('about.the_relay_address_changes_on_every_start_contacts')),
        ),

        card('🕸️', tr('about.relay_network'),
          p(tr('about.client_relays_form_a_distributed_network_to_help')),
          p(tr('about.if_a_contact_s_relay_is_unavailable_the')),
        ),

        card('🛡️', tr('about.protection_on_your_device'),
          list(
            [h('b', null, tr('about.encrypted_database')), tr('about.the_whole_profile_is_stored_in_sqlcipher_aes')],
            [h('b', null, tr('about.duress_passphrase')), tr('about.a_second_passphrase_opens_an_empty_profile_or')],
            [h('b', null, tr('about.attempt_limit')), tr('about.data_is_wiped_after_the_configured_number_of')],
            [h('b', null, tr('about.disappearing_messages')), tr('about.new_messages_can_expire_after_5_minutes_to')],
            [h('b', null, tr('about.keys_in_memory')), tr('about.secrets_are_cleared_after_use_and_process_memory')],
            [h('b', null, tr('about.contact_verification')), tr('about.compare_key_fingerprints_in_person_or_through_another')],
          ),
        ),

        card('⬆️', tr('about.updates'),
          p(tr('about.the_app_checks_github_releases_through_an_i2p')),
        ),

        card('💾', tr('about.moving_to_another_device'),
          steps(
            [tr('about.export'), tr('about.settings_backup_choose_a_backup_passphrase_of_at')],
            [tr('about.transfer'), tr('about.transfer_the_file_using_a_method_you_trust')],
            [tr('about.import'), tr('about.on_the_new_device_choose_import_backup_on')],
          ),
          h('div', { class: 'about-note warn' },
            tr('about.one_identity_means_one_active_device_after_importing')),
        ),

        card('⚖️', tr('about.known_limitations'),
          list(
            tr('about.there_has_been_no_independent_code_or_cryptography'),
            tr('about.this_is_a_young_project_bugs_are_possible'),
            tr('about.an_observer_with_visibility_over_the_whole_network'),
            tr('about.multiple_devices_sharing_one_identity_are_not_supported'),
            tr('about.delivery_depends_on_at_least_one_network_node'),
            tr('about.network_lookups_use_i2p_and_take_tens_of'),
          ),
        ),

        h('details', { class: 'about-tech' },
          h('summary', null, tr('about.cryptography_and_technology')),
          h('dl', { class: 'about-dl' },
            ...([
              [tr('about.identity'), tr('about.ed25519_signatures_x25519_key_agreement')],
              [tr('about.session_setup'), 'X3DH'],
              [tr('about.session'), 'Double Ratchet'],
              [tr('about.cipher'), 'XChaCha20-Poly1305'],
              [tr('about.key_derivation'), tr('about.hkdf_sha256_argon2id_for_passphrases')],
              [tr('about.database'), tr('about.sqlcipher_4_with_fts5_search_inside_the_encrypted')],
              [tr('about.transport'), tr('about.i2p_with_i2pd_embedded_in_the_same_process')],
              [tr('about.relay_network'), tr('about.kademlia_with_encrypted_and_signed_records')],
              [tr('about.language'), 'Rust'],
              [tr('about.avatars'), tr('about.details_from_prints_by_utagawa_kuniyoshi_and_utagawa')],
            ] as const).flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)]),
          ),
        ),
      ),
      h('div', { class: 'modal-footer' },
        h('button', { class: 'btn', onClick: close }, tr('about.got_it')),
      ),
    );
  }
}
