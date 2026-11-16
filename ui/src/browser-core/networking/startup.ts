/** Monotonic milestone timings. Values are elapsed milliseconds since this
 * attempt began; they do not imply that a later stage or tunnel is ready. */
export class StartupTimings {
  private start = performance.now();
  private marks: Record<string, number> = {};
  reset(): void { this.start = performance.now(); this.marks = {}; }
  mark(stage: string): void { if (!(stage in this.marks)) this.marks[stage] = Math.round(performance.now() - this.start); }
  snapshot(): Record<string, number> { return { ...this.marks }; }
}

export interface BootstrapJob { ready(): Promise<string>; close(): void; free(): void }
type BeginBootstrap = (url: string, digest: string) => BootstrapJob;

/** Only public RouterInfos enter this job. A selected profile is an ephemeral
 * cancellation scope, never an input to the verifier or a cache key. */
export class PublicPrewarm {
  private generation = 0;
  private key: string | null = null;
  private profile: string | undefined;
  private job: BootstrapJob | null = null;
  private result: Promise<void> | null = null;
  private state: 'off' | 'loading' | 'ready' | 'failed' = 'off';
  private receipt: unknown = null;
  constructor(private begin: BeginBootstrap) {}

  start(config: Record<string, string> | null, profile?: string): Promise<void> {
    if (!config) { this.cancel(); return Promise.resolve(); }
    const key = JSON.stringify([config.snapshot_url, config.snapshot_sha256]);
    if (key === this.key && this.result && (profile === undefined || this.profile === undefined || this.profile === profile)) {
      if (profile !== undefined) this.profile = profile;
      return this.result;
    }
    this.cancel();
    this.key = key; this.profile = profile;
    const generation = this.generation;
    this.state = 'loading';
    let job: BootstrapJob;
    try { job = this.begin(config.snapshot_url!, config.snapshot_sha256!); }
    catch { this.state = 'failed'; return Promise.resolve(); }
    this.job = job;
    this.result = job.ready().then(raw => {
      if (generation !== this.generation) return;
      this.receipt = JSON.parse(raw); this.state = 'ready';
    }).catch(() => {
      if (generation === this.generation) this.state = 'failed';
    }).finally(() => {
      if (this.job === job) { this.job = null; job.free(); }
    });
    return this.result;
  }
  cancel(): void {
    this.generation++;
    this.job?.close(); this.job?.free(); this.job = null;
    this.result = null; this.key = null; this.profile = undefined;
    this.state = 'off'; this.receipt = null;
  }
  /** The legacy prewarm indicator describes tunnels. Public seeds alone must
   * never return 'ready'; diagnostics name their distinct state explicitly. */
  status(): 'building' | 'off' { return this.state === 'loading' ? 'building' : 'off'; }
  diagnostics(): { state: string; receipt: unknown } { return { state: this.state, receipt: this.receipt }; }
}
