// C interface over libi2pd/api.h; see shim.h for the contract.

#include "shim.h"

#include <cstring>
#include <fstream>
#include <memory>
#include <string>
#include <vector>

#include <boost/asio.hpp>

#include "Destination.h"
#include "Identity.h"
#include "Streaming.h"
#include "Transports.h"
#include "api.h"
#include "util.h"

struct gipny_dest {
	std::shared_ptr<i2p::client::ClientDestination> dest;
};

struct gipny_stream {
	std::shared_ptr<i2p::stream::Stream> stream;
};

namespace {

char *dup_string(const std::string &s) {
	char *out = static_cast<char *>(std::malloc(s.size() + 1));
	if (!out) return nullptr;
	std::memcpy(out, s.data(), s.size());
	out[s.size()] = '\0';
	return out;
}

int error_code(const boost::system::error_code &ec) {
	if (!ec) return 0;
	if (ec == boost::asio::error::eof) return 1;
	if (ec == boost::asio::error::connection_reset) return 2;
	if (ec == boost::asio::error::timed_out) return 3;
	return 4;
}

// A full base64 destination, or "<base32>.b32.i2p".
bool parse_remote(const char *remote, i2p::data::IdentHash &out) {
	if (!remote) return false;
	std::string s(remote);
	const std::string suffix = ".b32.i2p";
	if (s.size() > suffix.size() && s.compare(s.size() - suffix.size(), suffix.size(), suffix) == 0) {
		std::string b32 = s.substr(0, s.size() - suffix.size());
		return out.FromBase32(b32) == 32;
	}
	i2p::data::IdentityEx ident;
	if (!ident.FromBase64(s)) return false;
	out = ident.GetIdentHash();
	return true;
}

} // namespace

extern "C" {

int gipny_router_init(int argc, const char *const *argv) {
	// InitI2P wants mutable strings, as main() gets them.
	std::vector<std::string> copies;
	copies.reserve(argc > 0 ? argc : 1);
	if (argc <= 0) copies.emplace_back("gipny");
	for (int i = 0; i < argc; ++i) copies.emplace_back(argv[i] ? argv[i] : "");
	std::vector<char *> args;
	for (auto &c : copies) args.push_back(c.data());
	args.push_back(nullptr);
	try {
		i2p::api::InitI2P(static_cast<int>(copies.size()), args.data(), "gipny-i2pd");
		return 1;
	} catch (...) {
		return 0;
	}
}

void gipny_router_start(const char *log_path) {
	std::shared_ptr<std::ostream> log;
	if (log_path && *log_path) {
		auto f = std::make_shared<std::ofstream>(log_path, std::ios::app);
		if (f->is_open()) log = f;
	}
	i2p::api::StartI2P(log);
}

void gipny_router_stop(void) {
	i2p::api::StopI2P();
	i2p::api::TerminateI2P();
}

void gipny_router_set_online(int online) {
	i2p::transport::transports.SetOnline(online != 0);
}

char *gipny_keys_generate(void) {
	auto keys = i2p::data::PrivateKeys::CreateRandomKeys(
		i2p::data::SIGNING_KEY_TYPE_EDDSA_SHA512_ED25519,
		i2p::data::CRYPTO_KEY_TYPE_ECIES_X25519_AEAD, true);
	return dup_string(keys.ToBase64());
}

char *gipny_keys_public(const char *private_b64) {
	if (!private_b64) return nullptr;
	i2p::data::PrivateKeys keys;
	if (!keys.FromBase64(private_b64)) return nullptr;
	return dup_string(keys.GetPublic()->ToBase64());
}

gipny_dest *gipny_dest_create(const char *private_b64, int publish,
                              const char *const *option_keys,
                              const char *const *option_values,
                              size_t options) {
	i2p::util::Mapping params;
	for (size_t i = 0; i < options; ++i) {
		if (option_keys[i] && option_values[i]) params.Insert(option_keys[i], option_values[i]);
	}
	std::shared_ptr<i2p::client::ClientDestination> dest;
	try {
		if (private_b64) {
			i2p::data::PrivateKeys keys;
			if (!keys.FromBase64(private_b64)) return nullptr;
			dest = i2p::api::CreateLocalDestination(keys, publish != 0, &params);
		} else {
			dest = i2p::api::CreateLocalDestination(publish != 0,
				i2p::data::SIGNING_KEY_TYPE_EDDSA_SHA512_ED25519, &params);
		}
	} catch (...) {
		return nullptr;
	}
	if (!dest) return nullptr;
	return new gipny_dest{dest};
}

void gipny_dest_destroy(gipny_dest *dest) {
	if (!dest) return;
	i2p::api::DestroyLocalDestination(dest->dest);
	delete dest;
}

int gipny_dest_is_ready(const gipny_dest *dest) {
	return dest && dest->dest && dest->dest->IsReady() ? 1 : 0;
}

char *gipny_dest_address(const gipny_dest *dest) {
	if (!dest || !dest->dest) return nullptr;
	return dup_string(dest->dest->GetIdentity()->ToBase64());
}

int gipny_dest_connect(gipny_dest *dest, const char *remote, uint16_t port,
                       gipny_stream_cb cb, void *ctx) {
	i2p::data::IdentHash hash;
	if (!dest || !dest->dest || !parse_remote(remote, hash)) return 0;
	dest->dest->CreateStream(
		[cb, ctx](std::shared_ptr<i2p::stream::Stream> stream) {
			cb(ctx, stream ? new gipny_stream{stream} : nullptr);
		},
		hash, port);
	return 1;
}

void gipny_dest_accept(gipny_dest *dest, gipny_stream_cb cb, void *ctx) {
	if (!dest || !dest->dest) return;
	dest->dest->AcceptStreams([cb, ctx](std::shared_ptr<i2p::stream::Stream> stream) {
		if (stream) cb(ctx, new gipny_stream{stream});
	});
}

void gipny_dest_stop_accepting(gipny_dest *dest) {
	if (dest && dest->dest) dest->dest->StopAcceptingStreams();
}

void gipny_stream_recv(gipny_stream *stream, uint8_t *buf, size_t len,
                       int timeout_secs, gipny_io_cb cb, void *ctx) {
	if (!stream || !stream->stream) {
		cb(ctx, 4, 0);
		return;
	}
	stream->stream->AsyncReceive(boost::asio::buffer(buf, len),
		[cb, ctx](const boost::system::error_code &ec, size_t n) {
			cb(ctx, error_code(ec), n);
		},
		timeout_secs);
}

void gipny_stream_send(gipny_stream *stream, const uint8_t *buf, size_t len,
                       gipny_io_cb cb, void *ctx) {
	if (!stream || !stream->stream) {
		cb(ctx, 4, 0);
		return;
	}
	// SendBuffer copies `buf` before AsyncSend returns.
	stream->stream->AsyncSend(buf, len,
		[cb, ctx, len](const boost::system::error_code &ec, size_t) {
			cb(ctx, error_code(ec), ec ? 0 : len);
		});
}

void gipny_stream_close(gipny_stream *stream) {
	if (stream && stream->stream) stream->stream->Close();
}

void gipny_stream_free(gipny_stream *stream) {
	delete stream;
}

void gipny_string_free(char *s) {
	std::free(s);
}

} // extern "C"
