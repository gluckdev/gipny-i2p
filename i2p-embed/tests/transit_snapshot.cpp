// Read the initialized router before StartI2P starts any network services.
#include <utility>
#include "RouterContext.h"
#include "Tunnel.h"

extern "C" uint64_t gipny_test_transit_snapshot(char *bandwidth_class, uint64_t *shared,
                                               uint32_t *max_tunnels, int *accepts) {
	const auto caps = i2p::context.GetRouterInfo().GetProperty("caps");
	*bandwidth_class = caps.empty() ? '\0' : caps.front();
	*shared = i2p::context.GetTransitBandwidthLimit();
	*max_tunnels = i2p::tunnel::tunnels.GetMaxNumTransitTunnels();
	*accepts = i2p::context.AcceptsTunnels();
	return i2p::context.GetBandwidthLimit();
}
