#!/usr/bin/env python3
"""
Отправляет служебное уведомление о новом релизе каждому контакту в профиле gipny.

Использование:
  notify-release.py <version> <release_url> <changelog_file>

Переменные окружения:
  GIPNY_PROFILE_DIR - путь к профилю gipny для отправки (обязательно)
  GIPNY_PASSWORD - пароль для разблокировки профиля (обязательно)
  GIPNY_AGENT_PATH - путь к gipny-agent (опционально, по умолчанию 'gipny-agent')
"""

import os
import sys
import subprocess
import tempfile
import json
import time
import sqlite3
from pathlib import Path

def get_contacts(profile_dir, password):
    """Получает список контактов из профиля."""
    db_path = Path(profile_dir) / "vault.db"
    if not db_path.exists():
        print(f"Warning: vault.db not found at {db_path}", file=sys.stderr)
        return []
    
    # База зашифрована, нужно использовать agent для получения списка
    # Создаём временный скрипт для получения контактов через agent CLI
    agent_path = os.getenv('GIPNY_AGENT_PATH', 'gipny-agent')
    
    try:
        result = subprocess.run(
            [agent_path, 'list-contacts', '--profile', str(profile_dir), '--password-stdin', '--json'],
            input=password.encode('utf-8'),
            capture_output=True,
            timeout=30
        )
        
        if result.returncode != 0:
            print(f"Warning: Failed to list contacts: {result.stderr.decode('utf-8', errors='replace')}", file=sys.stderr)
            return []
        
        contacts_data = json.loads(result.stdout.decode('utf-8'))
        return contacts_data.get('contacts', [])
    except Exception as e:
        print(f"Warning: Error getting contacts: {e}", file=sys.stderr)
        return []

def send_message(agent_path, profile_dir, password, recipient_address, message_file):
    """Отправляет сообщение одному получателю."""
    cmd = [
        agent_path,
        'send-message',
        '--profile', profile_dir,
        '--recipient', recipient_address,
        '--message-file', message_file,
        '--password-stdin'
    ]
    
    result = subprocess.run(
        cmd,
        input=password.encode('utf-8'),
        capture_output=True,
        timeout=120
    )
    
    return result.returncode == 0, result.stdout.decode('utf-8', errors='replace'), result.stderr.decode('utf-8', errors='replace')

def main():
    if len(sys.argv) != 4:
        print("Usage: notify-release.py <version> <release_url> <changelog_file>", file=sys.stderr)
        sys.exit(1)
    
    version = sys.argv[1]
    release_url = sys.argv[2]
    changelog_file = sys.argv[3]
    
    # Проверяем обязательные переменные окружения
    profile_dir = os.getenv('GIPNY_PROFILE_DIR')
    password = os.getenv('GIPNY_PASSWORD')
    agent_path = os.getenv('GIPNY_AGENT_PATH', 'gipny-agent')
    
    if not all([profile_dir, password]):
        print("Error: GIPNY_PROFILE_DIR and GIPNY_PASSWORD must be set", file=sys.stderr)
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
    
    print(f"=== Release notification for version {version} ===")
    print(f"Release URL: {release_url}")
    print(f"Message length: {len(message)} characters")
    print()
    
    # Получаем список контактов
    print("Fetching contacts from profile...")
    contacts = get_contacts(profile_dir, password)
    
    if not contacts:
        print("Warning: No contacts found in profile or failed to fetch contacts", file=sys.stderr)
        print("Skipping notification")
        sys.exit(0)
    
    # Фильтруем контакты: пропускаем ботов и заблокированных
    # trust: 0=unknown, 1=verified, 2=blocked
    # is_bot: True/False
    # request: 'none', 'incoming', 'outgoing'
    recipients = []
    for contact in contacts:
        # Пропускаем заблокированных (trust=2)
        if contact.get('trust', 0) == 2:
            continue
        # Пропускаем ботов
        if contact.get('is_bot', False):
            continue
        # Пропускаем входящие запросы (ещё не приняты)
        if contact.get('request', 'none') == 'incoming':
            continue
        
        recipients.append({
            'name': contact.get('name', 'Unknown'),
            'address': contact.get('i2p_address', contact.get('sign_pk', ''))
        })
    
    if not recipients:
        print("Warning: No eligible recipients found (all contacts are bots, blocked, or pending)")
        sys.exit(0)
    
    print(f"Found {len(recipients)} recipient(s):")
    for r in recipients:
        print(f"  - {r['name']}")
    print()
    
    # Создаём временный файл с сообщением
    with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False, encoding='utf-8') as tmp:
        tmp.write(message)
        tmp_path = tmp.name
    
    try:
        # Отправляем сообщение каждому контакту
        sent_count = 0
        failed_count = 0
        
        for i, recipient in enumerate(recipients, 1):
            print(f"[{i}/{len(recipients)}] Sending to {recipient['name']}...", end=' ', flush=True)
            
            success, stdout, stderr = send_message(
                agent_path, profile_dir, password, 
                recipient['address'], tmp_path
            )
            
            if success:
                print("✓")
                sent_count += 1
            else:
                print("✗")
                print(f"  Error: {stderr}", file=sys.stderr)
                failed_count += 1
            
            # Задержка между отправками, чтобы не перегружать роутер
            if i < len(recipients):
                time.sleep(2)
        
        print()
        print(f"=== Results ===")
        print(f"Sent: {sent_count}/{len(recipients)}")
        if failed_count > 0:
            print(f"Failed: {failed_count}")
            sys.exit(1)
        else:
            print("All notifications sent successfully!")
        
    finally:
        # Удаляем временный файл
        try:
            os.unlink(tmp_path)
        except:
            pass

if __name__ == '__main__':
    main()
