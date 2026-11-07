#ifndef GIPNY_UPNP_LIMITS_H
#define GIPNY_UPNP_LIMITS_H
#include <stdint.h>
/* miniwget's HTTP guard rejects a response once this clock has moved more than
 * 8 s since the exchange began. gipny_router_upnp_abort (shim.cpp) bumps the
 * epoch at shutdown and every bump moves the clock an hour ahead, so an
 * exchange already in flight gives up at its next received chunk while
 * exchanges started afterwards measure normally. */
#ifdef __cplusplus
extern "C" {
#endif
unsigned gipny_upnp_abort_epoch(void);
#ifdef __cplusplus
}
#endif
#define GIPNY_UPNP_ABORT_SKEW_MS 3600000ull
#ifdef _WIN32
#include <windows.h>
static uint64_t gipny_upnp_millis(void) {
    return GetTickCount64() + (uint64_t)gipny_upnp_abort_epoch() * GIPNY_UPNP_ABORT_SKEW_MS;
}
#else
#include <time.h>
static uint64_t gipny_upnp_millis(void) {
    struct timespec now;
    uint64_t skew = (uint64_t)gipny_upnp_abort_epoch() * GIPNY_UPNP_ABORT_SKEW_MS;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return skew;
    return (uint64_t)now.tv_sec * 1000 + (uint64_t)now.tv_nsec / 1000000 + skew;
}
#endif
#endif
