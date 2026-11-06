// Static build configuration, derived from upstream miniupnpcstrings.h.cmake.
// OS_STRING is a bare platform name: no version and no application name, so the
// HTTP User-Agent ("<OS> UPnP/1.1 MiniUPnPc/2.3.3") reads like any other
// miniupnpc client on the LAN and says nothing about which program sent it.
#ifndef MINIUPNPCSTRINGS_H_INCLUDED
#define MINIUPNPCSTRINGS_H_INCLUDED
#if defined(_WIN32)
#define OS_STRING "Windows"
#elif defined(__APPLE__)
#define OS_STRING "Darwin"
#else
#define OS_STRING "Linux"
#endif
#define MINIUPNPC_VERSION_STRING "2.3.3"
#define UPNP_VERSION_MAJOR 1
#define UPNP_VERSION_MINOR 1
#define UPNP_VERSION_STRING "UPnP/1.1"
#endif
