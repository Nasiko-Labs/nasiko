"""
Base Repository - Shared functionality for all repository modules
"""
import os
import base64
import cryptography.fernet
from abc import ABC


class BaseRepository(ABC):
    """Base repository class with shared functionality"""
    
    def __init__(self, db, logger):
        self.db = db
        self.logger = logger
        # Initialize encryption key for sensitive data
        self._encryption_key = self._get_or_create_encryption_key()
    
    def _get_or_create_encryption_key(self):
        """Get encryption key for sensitive data from USER_CREDENTIALS_ENCRYPTION_KEY environment variable only"""
        # Only get key from USER_CREDENTIALS_ENCRYPTION_KEY environment variable
        env_key = os.getenv("USER_CREDENTIALS_ENCRYPTION_KEY")
        if not env_key:
            error_msg = "USER_CREDENTIALS_ENCRYPTION_KEY environment variable is required but not found"
            self.logger.error(error_msg)
            raise ValueError(error_msg)
        
        try:
            return base64.urlsafe_b64decode(env_key.encode())
        except Exception as e:
            error_msg = f"Invalid encryption key in USER_CREDENTIALS_ENCRYPTION_KEY environment variable: {e}"
            self.logger.error(error_msg)
            raise ValueError(error_msg)
    
    def _encrypt_data(self, data: str) -> str:
        """Encrypt sensitive data"""
        if not data:
            return data
        
        try:
            fernet = cryptography.fernet.Fernet(self._encryption_key)
            encrypted_data = fernet.encrypt(data.encode())
            return base64.urlsafe_b64encode(encrypted_data).decode()
        except Exception as e:
            self.logger.error(f"Encryption failed: {e}")
            raise
    
    def _decrypt_data(self, encrypted_data: str) -> str:
        """Decrypt sensitive data"""
        if not encrypted_data:
            return encrypted_data
        
        try:
            encrypted_bytes = base64.urlsafe_b64decode(encrypted_data.encode())
            fernet = cryptography.fernet.Fernet(self._encryption_key)
            decrypted_data = fernet.decrypt(encrypted_bytes)
            return decrypted_data.decode()
        except Exception as e:
            self.logger.error(f"Decryption failed: {e}")
            raise