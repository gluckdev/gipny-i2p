// Mono box-filter decimator. Emit fixed-duration blocks even at fractional
// ratios such as 44.1 kHz -> 8 kHz. The output node stays silent.
class VoiceCapture extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.rate = options.processorOptions.rate;
    this.block = new Int16Array(this.rate / 4);
    this.offset = this.phase = this.sum = this.count = 0;
  }
  process(inputs) {
    for (const sample of inputs[0]?.[0] || []) {
      this.sum += sample;
      this.count++;
      this.phase += this.rate;
      if (this.phase < sampleRate) continue;
      this.phase -= sampleRate;
      this.block[this.offset++] = Math.round(Math.max(-1, Math.min(1, this.sum / this.count)) * 32767);
      this.sum = this.count = 0;
      if (this.offset === this.block.length) {
        this.port.postMessage(this.block, [this.block.buffer]);
        this.block = new Int16Array(this.rate / 4);
        this.offset = 0;
      }
    }
    return true;
  }
}
registerProcessor('voice-capture', VoiceCapture);
