// Execute with the target CPU, before accepting its release binary.
#include <atomic>
#include <compare>
#include <ctime>
#include <cstring>
#include <iostream>
#include <string>
#include <boost/program_options.hpp>
#include <openssl/evp.h>
#include <zlib.h>

int main() {
    static_assert(sizeof(time_t) == 8, "musl time64 ABI required");
    if ((1 <=> 2) != std::strong_ordering::less) return 1;
    std::atomic<unsigned long long> counter{0};
    if (++counter != 1 || std::stod("1.5") != 1.5) return 2;
    boost::program_options::options_description options("native probe");
    options.add_options()("check", "cross C++ runtime");
    const unsigned char input[] = {'a', 'b', 'c'};
    unsigned char digest[EVP_MAX_MD_SIZE];
    unsigned int length = 0;
    if (!EVP_Digest(input, sizeof(input), digest, &length, EVP_sha256(), nullptr) || length != 32) return 3;
    const unsigned char wanted[] = {0xba, 0x78, 0x16, 0xbf};
    if (std::memcmp(digest, wanted, sizeof(wanted))) return 4;
    unsigned char compressed[128], restored[128];
    uLongf compressed_size = sizeof(compressed), restored_size = sizeof(restored);
    if (compress(compressed, &compressed_size, input, sizeof(input)) != Z_OK ||
        uncompress(restored, &restored_size, compressed, compressed_size) != Z_OK ||
        restored_size != sizeof(input) || std::memcmp(restored, input, sizeof(input))) return 5;
    std::cout << "native C++20 / floating-point ABI / OpenSSL / zlib / Boost: PASS\n";
}
