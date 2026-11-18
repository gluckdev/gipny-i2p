import { focusVoiceDialog } from './dialog';
import type { App } from '../../app/app';
import { h } from '../../app/view';
import { t } from '../localization/i18n';
import { CLIP_RATE, CLIP_SECONDS, VoiceAudio, voiceFile, voiceTime } from './audio';

export function recordClip(app: App, attach: (file: File) => Promise<void>): () => void {
  return app.openModal(close => {
    let active = true, paused = false;
    let audio: VoiceAudio | undefined;
    let file: File | undefined;
    let url: string | undefined;
    const blocks: Int16Array[] = [];
    const status = h('p', { role: 'status' }, t('voice.mic'));
    const clock = h('time', { class: 'voice-time' }, '0:00 / ' + voiceTime(CLIP_SECONDS));
    const level = h('meter', { min: 0, max: 1, value: 0, 'aria-label': t('voice.level') });
    const monitor = h('div', { class: 'voice-monitor' }, clock, level);
    const pause = h('button', { class: 'btn btn-ghost', disabled: true, onClick: () => {
      paused = !paused;
      audio?.mute(paused);
      status.textContent = t(paused ? 'voice.paused' : 'voice.recording');
      if (paused) level.value = 0;
      pause.textContent = t(paused ? 'voice.resume' : 'voice.pause');
    } }, t('voice.pause'));
    const finish = () => {
      if (!audio || !blocks.length) return;
      audio.close(); audio = undefined;
      file = voiceFile(blocks);
      blocks.length = 0;
      url = URL.createObjectURL(file);
      status.textContent = t('voice.preview');
      monitor.replaceChildren(h('audio', { src: url, controls: true, 'aria-label': t('voice.preview') }));
      pause.remove();
      action.textContent = t('chat.send');
    };
    const action = h('button', { class: 'btn', disabled: true, onClick: async () => {
      if (!file) { finish(); return; }
      action.disabled = true;
      try { await attach(file); close(); } catch (error) { if (active) { status.textContent = String(error); action.disabled = false; } }
    } }, t('voice.stop'));
    const root = h('div', { class: 'modal voice-panel', role: 'dialog', 'aria-modal': true, 'aria-label': t('voice.clip') },
      h('h2', {}, t('voice.clip')), status, monitor,
      h('div', { class: 'voice-actions' }, h('button', { class: 'btn btn-ghost', onClick: close }, t('chat.cancel')), pause, action));
    root.addEventListener('gipny-modal-close', () => {
      active = false; audio?.close(); blocks.length = 0;
      if (url) URL.revokeObjectURL(url);
      window.removeEventListener('pagehide', close);
    });
    window.addEventListener('pagehide', close);
    void (async () => {
      try {
        audio = new VoiceAudio();
        await audio.start(CLIP_RATE, block => {
          if (!active || paused || file) return;
          blocks.push(block);
          if (blocks.length === 1) status.textContent = t('voice.recording');
          clock.textContent = voiceTime(blocks.length / 4) + ' / ' + voiceTime(CLIP_SECONDS);
          const energy = block.reduce((sum, sample) => sum + (sample / 32768) ** 2, 0);
          level.value = Math.min(1, Math.sqrt(energy / block.length) * 4);
          action.disabled = pause.disabled = false;
          if (blocks.length >= CLIP_SECONDS * 4) finish();
        });
      } catch (error) { audio?.close(); if (active) status.textContent = String(error); }
    })();
    return focusVoiceDialog(root, close);
  });
}
