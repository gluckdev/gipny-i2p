#!/usr/bin/env python3
import json
import re

files_to_parse = {
    "openvpn-tcp": "/home/artamonov/Downloads/mikhail.artamonov-tcp.ovpn",
    "openvpn-udp": "/home/artamonov/Downloads/mikhail.artamonov-udp.ovpn",
    "openvpn-udp-crypt": "/home/artamonov/Downloads/mikhail.artamonov-udp-crypt (1).ovpn"
}

def extract_tag(tag, text):
    match = re.search(f'<{tag}>(.*?)</{tag}>', text, re.DOTALL)
    return match.group(1).strip() if match else None

def parse_ovpn_file(tag, filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    ca = extract_tag('ca', content)
    cert = extract_tag('cert', content)
    key = extract_tag('key', content)
    tls_auth = extract_tag('tls-auth', content) or extract_tag('tls-crypt', content)

    directives = {}
    for line in content.split('\n'):
        line = line.strip()
        if not line or line.startswith('#') or line.startswith(';'):
            continue
        parts = line.split()
        directives[parts[0]] = parts[1:]

    remote = directives.get('remote', [])
    server = remote[0] if len(remote) > 0 else '127.0.0.1'
    server_port = int(remote[1]) if len(remote) > 1 else 1194

    proto = directives.get('proto', ['udp'])[0]
    cipher = directives.get('cipher', ['AES-256-GCM'])[0]
    auth = directives.get('auth', ['SHA256'])[0]
    key_direction = directives.get('key-direction', ['0'])[0]

    is_tcp = 'tcp' in proto.lower()

    outbound = {
        "type": "openvpn",
        "tag": tag,
        "server": server,
        "server_port": server_port,
        "tcp": is_tcp,
        "cipher": cipher,
        "auth": auth,
        "username": "YOUR_USERNAME",
        "password": "YOUR_PASSWORD"
    }

    if ca:
        outbound["ca"] = ca
    if cert:
        outbound["cert"] = cert
    if key:
        outbound["private_key"] = key
    if tls_auth:
        outbound["tls_auth"] = tls_auth
        outbound["tls_auth_key_direction"] = int(key_direction)

    return outbound

def main():
    outbounds = []
    tags = []
    
    for tag, path in files_to_parse.items():
        try:
            print(f"Parsing {path}...")
            outbound = parse_ovpn_file(tag, path)
            outbounds.append(outbound)
            tags.append(tag)
        except Exception as e:
            print(f"Failed to parse {path}: {e}")

    # Build the sing-box config
    config = {
        "log": {
            "level": "info",
            "timestamp": True
        },
        "inbounds": [
            {
                "type": "socks",
                "tag": "socks-in",
                "listen": "127.0.0.1",
                "listen_port": 1080
            },
            {
                "type": "http",
                "tag": "http-in",
                "listen": "127.0.0.1",
                "listen_port": 8080
            },
            {
                "type": "tun",
                "tag": "tun-in",
                "interface_name": "tun0",
                "address": [
                    "172.19.0.1/30",
                    "fdfe:dcba:9876::1/126"
                ],
                "auto_route": True,
                "strict_route": True,
                "stack": "system",
                "sniff": True
            }
        ],
        "outbounds": [
            # The URL Test Outbound which chooses the best connection
            {
                "type": "urltest",
                "tag": "balanced-vpn",
                "outbounds": tags,
                "url": "http://www.gstatic.com/generate_204",
                "interval": "1m",
                "tolerance": 50
            },
            *outbounds,
            {
                "type": "direct",
                "tag": "direct"
            }
        ],
        "route": {
            "auto_detect_interface": True,
            "rules": [
                {
                    "outbound": "balanced-vpn"
                }
            ]
        }
    }

    output_path = "/home/artamonov/Downloads/mikhail.artamonov-balanced.json"
    with open(output_path, "w") as f:
        json.dump(config, f, indent=2)

    print(f"\nGenerated balanced config successfully at: {output_path}")
    print("Please configure your username and password in the file.")

if __name__ == "__main__":
    main()
