"""Configure the child Cargo process Tauri launches from inside Xcode."""
import json
import os
from pathlib import Path

workspace = Path(os.environ["GITHUB_WORKSPACE"])
deps = workspace / ".ci-vcpkg" / "installed" / os.environ["IOS_TRIPLET"]
for rel in ("lib/libboost_program_options.a", "lib/libssl.a", "lib/libcrypto.a",
            "lib/libz.a", "include/openssl/crypto.h", "include/boost/asio.hpp"):
    if not (deps / rel).is_file():
        raise SystemExit(f"Missing iOS dependency {rel}")
prefix = os.environ["RUST_TARGET"].replace("-", "_").upper()
values = {
    "I2P_EMBED_INCLUDE_DIRS": str(deps / "include"),
    "I2P_EMBED_LIB_DIRS": str(deps / "lib"),
    "I2P_EMBED_STATIC": "1",
    "I2P_EMBED_CERTS_DIR": os.environ["I2P_EMBED_CERTS_DIR"],
    f"{prefix}_OPENSSL_DIR": str(deps),
    f"{prefix}_OPENSSL_NO_VENDOR": "1",
    f"{prefix}_OPENSSL_STATIC": "1",
}
# Xcode's Run Script phase rebuilds its environment. Cargo reads this workspace
# configuration even when the build phase did not inherit the shell variables.
with (workspace / ".cargo/config.toml").open("a", encoding="utf-8") as config:
    config.write("\n[env]\n")
    for name, value in values.items():
        config.write(f"{name} = {{ value = {json.dumps(value)}, force = true }}\n")
