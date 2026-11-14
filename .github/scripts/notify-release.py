#!/usr/bin/env python3
"""
Отправляет служебное уведомление о новом релизе в мессенджер gipny.

Использование:
  notify-release.py <version> <release_url> <changelog_file>

Переменные окружения:
  GIPNY_PROFILE_DIR - путь к профилю gipny для отправки (обязательно)
  GIPNY_RECIPIENT_ADDRESS - адрес получателя уведомлений (обязательно)
  GIPNY_PASSWORD - пароль для разблокировки профиля (обязательно)
"""

import os
import sys
import subprocess
import tempfile
import json
import time
from pathlib import Path

def main():
    if len(sys.argv) != 4:
        print("Usage: notify-release.py <version> <release_url> <changelog_file>", file=sys.stderr)
        sys.exit(1)
    
    version = sys.argv[1]
    release_url = sys.argv[2]
    changelog_file = sys.argv[3]
    
    # Проверяем обязательные переменные окружения
    profile_dir = os.getenv('GIPNY_PROFILE_DIR')
    recipient = os.getenv('GIPNY_RECIPIENT_ADDRESS')
    password = os.getenv('GIPNY_PASSWORD')
    
    if not all([profile_dir, recipient, password]):
        print("Error: GIPNY_PROFILE_DIR, GIPNY_RECIPIENT_ADDRESS and GIPNY_PASSWORD must be set", file=sys.stderr)
        sys.exit(1)
    
    if not os.path.isdir(profile_dir):
        print(f"Error: Profile directory not found: {profile_dir}", file=sys.stderr)
        sys.exit(1)
    
    if not os.path.isfile(changelog_file):
        print(f"Error: Changelog file not found: {changelog_file}", file=sys.stderr)
        sys.exit(1)
    
    # Читаем changelog
    with open(changelog_file, 'r', encoding='utf-8') as f:
        changelog = f.read().strip()
    
    # Формируем текст сообщения
    message = f"""🚀 Новый релиз gipny-i2p {version}

{changelog}

Скачать: {release_url}

---
Автоматическое уведомление о релизе"""
    
    print(f"Preparing to send release notification for version {version}")
    print(f"Recipient: {recipient}")
    print(f"Message length: {len(message)} characters")
    
    # Для отправки нужен запущенный агент gipny
    # Используем agent CLI для отправки сообщения
    # Это простейший вариант - можно также использовать libcore напрямую
    
    # Создаём временный файл с сообщением
    with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False, encoding='utf-8') as tmp:
        tmp.write(message)
        tmp_path = tmp.name
    
    try:
        # Запускаем gipny agent для отправки
        # agent send-message --profile <profile> --recipient <address> --message-file <file>
        cmd = [
            'gipny-agent',
            'send-message',
            '--profile', profile_dir,
            '--recipient', recipient,
            '--message-file', tmp_path,
            '--password-stdin'
        ]
        
        print(f"Executing: {' '.join(cmd)}")
        
        result = subprocess.run(
            cmd,
            input=password.encode('utf-8'),
            capture_output=True,
            timeout=120
        )
        
        if result.returncode != 0:
            print(f"Error: Failed to send message", file=sys.stderr)
            print(f"stdout: {result.stdout.decode('utf-8', errors='replace')}", file=sys.stderr)
            print(f"stderr: {result.stderr.decode('utf-8', errors='replace')}", file=sys.stderr)
            sys.exit(1)
        
        print("Release notification sent successfully")
        print(result.stdout.decode('utf-8', errors='replace'))
        
    finally:
        # Удаляем временный файл
        try:
            os.unlink(tmp_path)
        except:
            pass

if __name__ == '__main__':
    main()
