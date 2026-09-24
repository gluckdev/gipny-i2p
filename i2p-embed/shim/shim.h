/*
 * The i2pd router inside our process: a C interface over libi2pd/api.h.
 *
 * No SAM, no TCP, no local port of any kind. The Rust side (src/lib.rs) is
 * the only caller. Every callback runs on one of the router's own threads
 * (a destination's io_context), never on the caller's; the Rust side only
 * wakes a task from it.
 *
 * Ownership: every gipny_* object returned is released with its own
 * *_free/*_destroy; strings returned are released with gipny_string_free.
 */
#ifndef GIPNY_I2P_SHIM_H
#define GIPNY_I2P_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct gipny_dest gipny_dest;
typedef struct gipny_stream gipny_stream;

/* Router lifecycle. `argv` are i2pd's own options ("--datadir=…",
 * "--bandwidth=…"); argv[0] is ignored. Call once per process. */
int gipny_router_init(int argc, const char *const *argv);
/* `log_path` NULL logs to <datadir>/gipny-i2pd.log. */
void gipny_router_start(const char *log_path);
void gipny_router_stop(void);
/* Tell the router the machine's network changed (Transports::SetOnline). */
void gipny_router_set_online(int online);

/* Fresh keys: base64 private keys (what DestinationKind::Persistent held). */
char *gipny_keys_generate(void);
/* The public destination (base64) of base64 private keys; NULL if invalid. */
char *gipny_keys_public(const char *private_b64);

/* A local destination. `private_b64` NULL makes a transient one. `publish` 0
 * keeps its LeaseSet off the network (outbound only). Options are i2cp
 * tunnel parameters, e.g. "inbound.length" → "2". NULL on bad keys. */
gipny_dest *gipny_dest_create(const char *private_b64, int publish,
                              const char *const *option_keys,
                              const char *const *option_values,
                              size_t options);
void gipny_dest_destroy(gipny_dest *dest);
/* Its LeaseSet is up and it has outbound tunnels. */
int gipny_dest_is_ready(const gipny_dest *dest);
/* Base64 public destination. */
char *gipny_dest_address(const gipny_dest *dest);

/* `stream` NULL means the attempt failed (no LeaseSet found, no tunnels). */
typedef void (*gipny_stream_cb)(void *ctx, gipny_stream *stream);
/* Open a stream to `remote`: a full base64 destination or a "….b32.i2p". The
 * callback runs exactly once. Returns 0 if `remote` does not parse (then the
 * callback does not run). */
int gipny_dest_connect(gipny_dest *dest, const char *remote, uint16_t port,
                       gipny_stream_cb cb, void *ctx);
/* Deliver every inbound stream to `cb` until gipny_dest_stop_accepting or
 * destroy. `ctx` must outlive that. */
void gipny_dest_accept(gipny_dest *dest, gipny_stream_cb cb, void *ctx);
void gipny_dest_stop_accepting(gipny_dest *dest);

/* `error`: 0 ok, 1 end of stream, 2 reset, 3 timed out, 4 other. */
typedef void (*gipny_io_cb)(void *ctx, int error, size_t bytes);
/* Read up to `len` bytes into `buf`, which must stay valid until the
 * callback. At most one read in flight per stream. Waits up to
 * `timeout_secs` for data (then error 3). */
void gipny_stream_recv(gipny_stream *stream, uint8_t *buf, size_t len,
                       int timeout_secs, gipny_io_cb cb, void *ctx);
/* Queue `len` bytes; they are copied before this returns. The callback runs
 * once they are handed to the network (or failed). */
void gipny_stream_send(gipny_stream *stream, const uint8_t *buf, size_t len,
                       gipny_io_cb cb, void *ctx);
void gipny_stream_close(gipny_stream *stream);
void gipny_stream_free(gipny_stream *stream);

void gipny_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif
