#!/usr/bin/env python3
"""
Superuser initialization script for Docker Compose setup
"""
import sys
import time
import os

# Add orchestrator to path
sys.path.insert(0, '/app/orchestrator')

def main():
    print('⏳ Waiting for services to be ready...')
    time.sleep(5)
    
    print('👤 Creating superuser...')
    try:
        from superuser_manager import SuperuserManager
        
        manager = SuperuserManager(auth_service_url='http://nasiko-auth-service:8001')
        user_id = manager.ensure_superuser()
        
        if user_id:
            print(f'✅ Superuser created/verified successfully: {user_id}')
            print(f'📧 Email: {manager.superuser_email}')
            print(f'👤 Username: {manager.superuser_username}')
            print('🎉 Superuser setup complete!')
            return 0
        else:
            print('❌ Superuser creation failed')
            return 1
            
    except Exception as e:
        print(f'❌ Superuser creation error: {e}')
        return 1

if __name__ == '__main__':
    sys.exit(main())