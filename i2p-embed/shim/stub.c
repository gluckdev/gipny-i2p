/*
 * The shim's interface with no router behind it, for builds that must link but
 * carry no router: build.yml's debug APKs ("no-router"), which check that the
 * app compiles, installs and starts on every push without an hour of boost for
 * Android. The router never starts (gipny_router_init fails), and everything
 * else is unreachable without one. Selected by I2P_EMBED_STUB=1 in build.rs;
 * never in anything released.
 */
#include "shim.h"

#include <stdlib.h>

int gipny_router_init(int argc, const char *const *argv) { (void)argc; (void)argv; return 0; }
void gipny_router_start(const char *log_path) { (void)log_path; }
void gipny_router_stop(void) {}
void gipny_router_set_online(int online) { (void)online; }
char *gipny_keys_generate(void) { return NULL; }
char *gipny_keys_public(const char *private_b64) { (void)private_b64; return NULL; }
gipny_dest *gipny_dest_create(const char *private_b64, int publish, const char *const *option_keys,
                              const char *const *option_values, size_t options) {
	(void)private_b64; (void)publish; (void)option_keys; (void)option_values; (void)options;
	return NULL;
}
void gipny_dest_destroy(gipny_dest *dest) { (void)dest; }
int gipny_dest_is_ready(const gipny_dest *dest) { (void)dest; return 0; }
char *gipny_dest_address(const gipny_dest *dest) { (void)dest; return NULL; }
int gipny_dest_connect(gipny_dest *dest, const char *remote, uint16_t port, gipny_stream_cb cb, void *ctx) {
	(void)dest; (void)remote; (void)port; (void)cb; (void)ctx;
	return 0;
}
void gipny_dest_accept(gipny_dest *dest, gipny_stream_cb cb, void *ctx) { (void)dest; (void)cb; (void)ctx; }
void gipny_dest_stop_accepting(gipny_dest *dest) { (void)dest; }
void gipny_stream_recv(gipny_stream *stream, uint8_t *buf, size_t len, int timeout_secs, gipny_io_cb cb, void *ctx) {
	(void)stream; (void)buf; (void)len; (void)timeout_secs;
	cb(ctx, 4, 0);
}
void gipny_stream_send(gipny_stream *stream, const uint8_t *buf, size_t len, gipny_io_cb cb, void *ctx) {
	(void)stream; (void)buf; (void)len;
	cb(ctx, 4, 0);
}
void gipny_stream_close(gipny_stream *stream) { (void)stream; }
void gipny_stream_free(gipny_stream *stream) { (void)stream; }
void gipny_string_free(char *s) { free(s); }
