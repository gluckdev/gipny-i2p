// Test-only adapter: allow loopback before any i2pd transport threads start.
// Compiled only with browser-router's explicit native-peer feature.
#include "Transports.h"

extern "C" int gipny_router_init(int argc, const char *const *argv);
extern "C" void gipny_router_start(const char *log_path);

extern "C" int gipny_browser_fixture_start(int argc, const char *const *argv) {
    if (!gipny_router_init(argc, argv)) return 0;
    i2p::transport::transports.SetCheckReserved(false);
    gipny_router_start(nullptr);
    return 1;
}
