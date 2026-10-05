#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# Gipny Web Deployment Script for GCP Compute Engine (deusexxx.online)
# ==============================================================================

echo "=== [1/6] Installing System Dependencies (Nginx, SSL tools) ==="
sudo apt-get update -y
sudo apt-get install -y nginx curl ufw

echo "=== [2/6] Creating gipny system user and data directories ==="
if ! id -u gipny >/dev/null 2>&1; then
    sudo useradd -r -s /bin/false -d /var/lib/gipny gipny
fi

sudo mkdir -p /var/lib/gipny
sudo mkdir -p /var/www/gipny-web
sudo mkdir -p /etc/ssl/certs /etc/ssl/private

echo "=== [3/6] Setting directory permissions ==="
sudo chown -R gipny:gipny /var/lib/gipny
sudo chmod 700 /var/lib/gipny

echo "=== [4/6] Installing Nginx configuration for deusexxx.online ==="
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sudo cp "$SCRIPT_DIR/nginx-deusexxx.online.conf" /etc/nginx/sites-available/deusexxx.online
sudo ln -sf /etc/nginx/sites-available/deusexxx.online /etc/nginx/sites-enabled/deusexxx.online
sudo rm -f /etc/nginx/sites-enabled/default

echo "=== [5/6] Installing systemd service ==="
sudo cp "$SCRIPT_DIR/gipny-web.service" /etc/systemd/system/gipny-web.service
sudo systemctl daemon-reload

echo "=== [6/6] Verifying SSL Certificates ==="
if [ ! -f /etc/ssl/certs/cloudflare-origin.pem ] || [ ! -f /etc/ssl/private/cloudflare-origin.key ]; then
    echo "----------------------------------------------------------------------------------"
    echo "ATTENTION: Cloudflare Origin Certificate not found!"
    echo "1. Go to Cloudflare Dashboard -> SSL/TLS -> Origin Server -> Create Certificate"
    echo "2. Save Origin Certificate to: /etc/ssl/certs/cloudflare-origin.pem"
    echo "3. Save Private Key to:        /etc/ssl/private/cloudflare-origin.key"
    echo "4. Set permissions:            sudo chmod 600 /etc/ssl/private/cloudflare-origin.key"
    echo "5. Restart Nginx:              sudo systemctl restart nginx"
    echo "6. Start Gipny Web:            sudo systemctl enable --now gipny-web"
    echo "----------------------------------------------------------------------------------"
else
    sudo systemctl restart nginx
    echo "Nginx configured and restarted successfully."
fi

echo "=== Deployment files installed successfully! ==="
