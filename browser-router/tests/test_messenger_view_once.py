"""The live gate must wait for the peer without resending or extending its budget."""
import unittest
from unittest.mock import patch

import run_messenger_ci as messenger


class ViewOnceGateTests(unittest.TestCase):
    def setUp(self):
        self.now = 0
        self.queue_at = 0
        self.notice_at = 4
        self.sent = 0
        self.opened = False
        self.notice = {'id': 'letter', 'contact_id': 22, 'sent_at': 1, 'expires_at': 300001}
        self.enterContext(patch.object(messenger.time, 'monotonic', lambda: self.now))
        self.enterContext(patch.object(messenger.time, 'sleep', self.sleep))
        self.enterContext(patch.object(messenger, 'evaluate', self.evaluate))
        self.enterContext(patch.object(messenger, 'rpc', self.rpc))

    def sleep(self, seconds):
        self.now += seconds

    def evaluate(self, client, expression):
        if "'send_view_once'" in expression:
            self.assertEqual(client, 'alice')
            self.sent += 1
            return
        if expression == 'globalThis.__viewOnceSent || null':
            return {'id': 'letter'} if self.now >= self.queue_at else None
        self.assertEqual(client, 'bob')
        self.assertIn("'open_view_once'", expression)
        self.opened = True
        return True

    def rpc(self, client, command, *, contactId):
        if command == 'list_messages':
            return []
        self.assertEqual((client, command, contactId), ('bob', 'list_view_once', 22))
        if self.now >= self.notice_at and not self.opened:
            return [self.notice]
        return [{'id': 'another-letter'}]

    def test_peer_notice_can_appear_after_enqueue(self):
        messenger.check_view_once('alice', 'bob', 11, 22)
        self.assertEqual((self.sent, self.opened, self.now), (1, True, 4))

    def test_enqueue_and_peer_share_the_original_deadline(self):
        self.queue_at = 200
        self.notice_at = 232
        with self.assertRaisesRegex(RuntimeError, 'deadline: view-once recipient notice'):
            messenger.check_view_once('alice', 'bob', 11, 22)
        self.assertEqual((self.sent, self.opened, self.now), (1, False, 230))

    def test_notice_exposing_content_fails_before_open(self):
        self.notice['body'] = 'unexpected content'
        with self.assertRaisesRegex(RuntimeError, 'metadata-only notice'):
            messenger.check_view_once('alice', 'bob', 11, 22)
        self.assertFalse(self.opened)
