"""
Superuser Manager
Handles creation and management of the system superuser.
"""

import os
import time
import logging
import requests
import json
from typing import Optional

logger = logging.getLogger(__name__)

class SuperuserManager:
    """Manages superuser creation and retrieval"""
    
    def __init__(self, auth_service_url: str = "http://localhost:8082"):
        self.auth_service_url = auth_service_url.rstrip('/')
        
        # Superuser configuration from environment variables
        self.superuser_email = os.getenv("SUPERUSER_EMAIL", "admin@nasiko.com")
        self.superuser_username = os.getenv("SUPERUSER_USERNAME", "admin")  # Use as username
        self.superuser_password = os.getenv("SUPERUSER_PASSWORD", "admin123")  # Not used in this auth system
        
    def wait_for_auth_service(self, max_attempts: int = 30) -> bool:
        """Wait for auth service to be ready"""
        logger.info("Waiting for auth service to be ready...")
        
        for attempt in range(max_attempts):
            try:
                response = requests.get(f"{self.auth_service_url}/health", timeout=5)
                if response.status_code == 200:
                    logger.info("Auth service is ready")
                    return True
            except requests.RequestException:
                pass
            
            if attempt < max_attempts - 1:
                logger.info(f"Auth service not ready, attempt {attempt + 1}/{max_attempts}, retrying in 2s...")
                time.sleep(2)
        
        logger.error("Auth service failed to become ready")
        return False
    
    def check_user_exists(self, username: str) -> bool:
        """Check if a user exists by username"""
        try:
            response = requests.post(
                f"{self.auth_service_url}/auth/users/check",
                json={"username": username},
                timeout=10
            )
            if response.status_code == 200:
                data = response.json()
                return data.get("exists", False)
            return False
        except requests.RequestException as e:
            logger.error(f"Failed to check if user exists: {e}")
            return False
    
    def create_superuser(self) -> Optional[str]:
        """Create superuser and return user ID"""
        logger.info(f"Creating superuser: {self.superuser_username}")
        
        try:
            response = requests.post(
                f"{self.auth_service_url}/auth/users/register",
                json={
                    "username": self.superuser_username,
                    "email": self.superuser_email,
                    "is_super_user": True
                },
                timeout=10
            )
            
            if response.status_code in [200, 201]:
                data = response.json()
                user_id = data.get('user_id')
                access_key = data.get('access_key')
                access_secret = data.get('access_secret')
                
                if user_id and access_key and access_secret:
                    logger.info(f"Superuser created successfully with ID: {user_id}")
                    
                    # Save credentials to file
                    self.save_credentials_to_file(user_id, access_key, access_secret)
                    
                    return user_id
                else:
                    logger.error("Superuser created but no user_id returned")
                    logger.error(f"Response: {data}")
            elif response.status_code == 400:
                # User might already exist
                error_text = response.text
                if "already exists" in error_text.lower():
                    logger.info("Superuser already exists, checking for user_id...")
                    # Since we can't get the ID easily, let's return a placeholder and handle it in the orchestrator
                    # The orchestrator should still work even without the exact user_id for existing users
                    return "existing_user"
                else:
                    logger.error(f"Failed to create superuser: {response.status_code}")
                    logger.error(f"Response: {response.text}")
            else:
                logger.error(f"Failed to create superuser: {response.status_code}")
                logger.error(f"Response: {response.text}")
                
        except requests.RequestException as e:
            logger.error(f"Failed to create superuser: {e}")
        
        return None
    
    def get_superuser_id(self) -> Optional[str]:
        """Get existing superuser ID by logging in"""
        try:
            # Login to get token
            login_response = requests.post(
                f"{self.auth_service_url}/auth/login",
                json={
                    "email": self.superuser_email,
                    "password": self.superuser_password
                },
                timeout=10
            )
            
            if login_response.status_code != 200:
                logger.error("Failed to login as superuser")
                return None
            
            login_data = login_response.json()
            token = login_data.get('access_token')
            
            if not token:
                logger.error("No access token in login response")
                return None
            
            # Get user profile
            profile_response = requests.get(
                f"{self.auth_service_url}/users/profile",
                headers={"Authorization": f"Bearer {token}"},
                timeout=10
            )
            
            if profile_response.status_code == 200:
                profile_data = profile_response.json()
                user_id = profile_data.get('user_id') or profile_data.get('id')
                if user_id:
                    logger.info(f"Retrieved existing superuser ID: {user_id}")
                    return user_id
            
            logger.error("Failed to get superuser profile")
            
        except requests.RequestException as e:
            logger.error(f"Failed to get superuser ID: {e}")
        
        return None
    
    def ensure_superuser(self) -> Optional[str]:
        """Ensure superuser exists and return user ID"""
        logger.info("Ensuring superuser exists...")
        
        # Wait for auth service to be ready
        if not self.wait_for_auth_service():
            return None
        
        # Check if superuser already exists  
        user_check_response = self.check_user_exists(self.superuser_username)
        
        if user_check_response:
            logger.info(f"Superuser already exists: {self.superuser_username}")
            # Get user_id from check response if available, otherwise we'll need to handle this differently
            # For now, let's try to create and handle the "user exists" error
        
        logger.info(f"Attempting to create superuser: {self.superuser_username}")
        user_id = self.create_superuser()
        
        if user_id:
            logger.info("Superuser creation completed successfully")
            logger.info(f"Username: {self.superuser_username}, Email: {self.superuser_email}")
        
        return user_id
    
    def save_credentials_to_file(self, user_id: str, access_key: str, access_secret: str) -> None:
        """Save superuser credentials to a file for later use"""
        try:
            credentials = {
                "user_id": user_id,
                "username": self.superuser_username,
                "email": self.superuser_email,
                "access_key": access_key,
                "access_secret": access_secret,
                "is_super_user": True,
                "created_at": time.strftime("%Y-%m-%d %H:%M:%S")
            }
            
            # Save to orchestrator directory
            credentials_file = os.path.join(os.path.dirname(__file__), "superuser_credentials.json")
            
            with open(credentials_file, 'w') as f:
                json.dump(credentials, f, indent=2)
            
            logger.info(f"Superuser credentials saved to: {credentials_file}")
            logger.info(f"Access Key: {access_key}")
            logger.info("Access Secret: [HIDDEN] - check file for full secret")
            
        except Exception as e:
            logger.error(f"Failed to save credentials to file: {e}")
    
    def get_superuser_credentials(self) -> dict:
        """Get superuser credentials for reference"""
        return {
            "email": self.superuser_email,
            "username": self.superuser_username,
            "password": self.superuser_password
        }