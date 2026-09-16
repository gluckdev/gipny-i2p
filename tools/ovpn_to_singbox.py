#!/usr/bin/env python3
import sys
import json
import re

def parse_ovpn(file_path):
    with open(file_path, 'r') as f:
        content = f.read()

    # Simple regex to extract XML-like tags
    def extract_tag(tag, text):
        match = re.search(f'<{tag}>(.*?)</{tag}>', text, re.DOTALL)
        return match.group(1).strip() if match else None

    ca = extract_tag('ca', content)
    cert = extract_tag('cert', content)
    key = extract_tag('key', content)
    tls_auth = extract_tag('tls-auth', content) or extract_tag('tls-crypt', content)

    # Parse regular directives
    directives = {}
    for line in content.split('\n'):
        line = line.strip()
        if not line or line.startswith('#') or line.startswith(';'):
            continue
        parts = line.split()
        directives[parts[0]] = parts[1:]

    # Extract remote server and port
    remote = directives.get('remote', [])
    server = remote[0] if len(remote) > 0 else '127.0.0.1'
    server_port = int(remote[1]) if len(remote) > 1 else 1194

    proto = directives.get('proto', ['udp'])[0]
    cipher = directives.get('cipher', ['AES-256-GCM'])[0]
    auth = directives.get('auth', ['SHA256'])[0]
    key_direction = directives.get('key-direction', ['0'])[0]

    # Map proto
    is_tcp = 'tcp' in proto.lower()

    # Build sing-box outbound config
    outbound = {
        "type": "openvpn",
        "tag": "openvpn-out",
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

    # Build the full sing-box config
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
            }
        ],
        "outbounds": [
            outbound,
            {
                "type": "direct",
                "tag": "direct"
            }
        ],
        "route": {
            "rules": [
                {
                    "outbound": "openvpn-out"
                }
            ]
        }
    }

    return config

def main():
    if len(sys.argv) < 3:
        print("Usage: ./ovpn_to_singbox.py <input.ovpn> <output.json>")
        sys.exit(1)

    input_file = sys.argv[1]
    output_file = sys.argv[2]

    try:
        config = parse_ovpn(input_file)
        with open(output_file, 'w') as f:
            json.dump(config, f, indent=2)
        print(f"Successfully converted {input_file} to {output_file}")
        print("Please edit the output file and replace YOUR_USERNAME and YOUR_PASSWORD with your actual VPN credentials.")
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)

if __name__ == '__main__':
    main()
