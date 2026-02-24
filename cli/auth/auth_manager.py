"""
Secure Authentication Manager for Nasiko CLI.
Provides secure token storage using OS keyring with fallback to encrypted files.
"""

import json
from typing import Optional, Dict, Any
import requests
import typer

# Try to import keyring for secure storage
try:
    import keyring
    KEYRING_AVAILABLE = True
except ImportError:
    KEYRING_AVAILABLE = False
    typer.echo("⚠️  Warning: keyring not available. Falling back to file-based storage.")

# Fallback imports for encrypted storage
try:
    from cryptography.fernet import Fernet
    import base64
    import hashlib
    CRYPTO_AVAILABLE = True
except ImportError:
    CRYPTO_AVAILABLE = False

import sys
import os

# Add CLI directory to path for imports
cli_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if cli_dir not in sys.path:
    sys.path.insert(0, cli_dir)

from core.settings import CONFIG_DIR
from setup.config import get_cluster_api_url


class AuthManager:
    """Secure authentication manager for Nasiko CLI"""
    
    SERVICE_NAME = "nasiko-cli"
    TOKEN_KEY = "jwt_token"
    CREDS_KEY = "user_creds"
    
    def __init__(self, base_url: str = None, cluster_name: str = None):
        self.config_dir = CONFIG_DIR
        self.config_dir.mkdir(parents=True, exist_ok=True)
        
        # Determine Base URL
        if base_url:
            self.base_url = base_url.rstrip('/')
        elif cluster_name:
            # Look up URL for specific cluster
            url = get_cluster_api_url(cluster_name)
            if url:
                self.base_url = url.rstrip('/')
            else:
                 # Fallback to localhost if cluster unknown (shouldn't happen if validated before)
                 self.base_url = "http://localhost:8000"
        else:
            # Fallback 1: NASIKO_API_URL env var
            env_url = os.getenv("NASIKO_API_URL")
            if env_url:
                self.base_url = env_url.rstrip('/')
            else:
                # Check for NASIKO_CLUSTER_NAME env var
                default_cluster = os.environ.get("NASIKO_CLUSTER_NAME")
                if default_cluster:
                    url = get_cluster_api_url(default_cluster)
                    if url:
                        self.base_url = url.rstrip('/')
                    else:
                         self.base_url = "http://localhost:8000"
                else:
                    # Fallback 2: Default to localhost
                    self.base_url = "http://localhost:8000"
        
        self.auth_url = self.base_url  # Auth service is at base URL
        
        # File-based fallback paths
        self.token_file = self.config_dir / "token.enc"
        self.creds_file = self.config_dir / "credentials.enc"
        
    def _get_encryption_key(self) -> bytes:
        """Generate encryption key from system/user information"""
        if not CRYPTO_AVAILABLE:
            raise RuntimeError("Cryptography not available for secure storage")
        
        # Use a combination of username and machine info for key derivation
        import getpass
        import platform
        
        user_info = f"{getpass.getuser()}:{platform.node()}:{platform.system()}"
        key = hashlib.pbkdf2_hmac('sha256', user_info.encode(), b'nasiko-cli-salt', 100000)
        return base64.urlsafe_b64encode(key)
    
    def _encrypt_data(self, data: str) -> bytes:
        """Encrypt data using system-derived key"""
        if not CRYPTO_AVAILABLE:
            return data.encode()
        
        key = self._get_encryption_key()
        f = Fernet(key)
        return f.encrypt(data.encode())
    
    def _decrypt_data(self, encrypted_data: bytes) -> str:
        """Decrypt data using system-derived key"""
        if not CRYPTO_AVAILABLE:
            return encrypted_data.decode()
        
        try:
            key = self._get_encryption_key()
            f = Fernet(key)
            return f.decrypt(encrypted_data).decode()
        except Exception:
            raise ValueError("Failed to decrypt data - key may have changed")
    
    def _store_secure(self, key: str, value: str) -> bool:
        """Store value securely using best available method"""
        if KEYRING_AVAILABLE:
            try:
                keyring.set_password(self.SERVICE_NAME, key, value)
                return True
            except Exception as e:
                typer.echo(f"⚠️  Keyring storage failed: {e}")
                
        # Fallback to encrypted file storage
        try:
            if key == self.TOKEN_KEY:
                file_path = self.token_file
            else:
                file_path = self.creds_file
                
            encrypted_data = self._encrypt_data(value)
            file_path.write_bytes(encrypted_data)
            file_path.chmod(0o600)  # Read-write for owner only
            return True
        except Exception as e:
            typer.echo(f"❌ Failed to store {key} securely: {e}")
            return False
    
    def _retrieve_secure(self, key: str) -> Optional[str]:
        """Retrieve value securely using best available method"""
        if KEYRING_AVAILABLE:
            try:
                value = keyring.get_password(self.SERVICE_NAME, key)
                if value:
                    return value
            except Exception as e:
                typer.echo(f"⚠️  Keyring retrieval failed: {e}")
        
        # Fallback to encrypted file storage
        try:
            if key == self.TOKEN_KEY:
                file_path = self.token_file
            else:
                file_path = self.creds_file
                
            if file_path.exists():
                encrypted_data = file_path.read_bytes()
                return self._decrypt_data(encrypted_data)
        except Exception as e:
            typer.echo(f"⚠️  Failed to retrieve {key}: {e}")
        
        return None
    
    def _delete_secure(self, key: str) -> bool:
        """Delete stored value securely"""
        success = True
        
        if KEYRING_AVAILABLE:
            try:
                keyring.delete_password(self.SERVICE_NAME, key)
            except Exception:
                pass  # May not exist
        
        # Also remove file-based storage
        try:
            if key == self.TOKEN_KEY:
                file_path = self.token_file
            else:
                file_path = self.creds_file
                
            if file_path.exists():
                file_path.unlink()
        except Exception:
            success = False
            
        return success
    
    def login(self, access_key: str, access_secret: str, save_credentials: bool = True) -> bool:
        """Login and store JWT token securely"""
        try:
            # Make login request
            base_url_str = str(self.base_url)  # Ensure it's a string
            login_url = f"{self.auth_url}/auth/users/login"
            response = requests.post(login_url, json={
                "access_key": access_key,
                "access_secret": access_secret
            }, timeout=30)
            
            if response.status_code == 200:
                token_data = response.json()
                jwt_token = token_data["token"]
                
                # Store JWT token securely
                if self._store_secure(self.TOKEN_KEY, jwt_token):
                    typer.echo("✅ Login successful! Token stored securely.")
                    
                    # Optionally store credentials for auto-renewal
                    if save_credentials:
                        creds = json.dumps({
                            "access_key": access_key,
                            "access_secret": access_secret
                        })
                        self._store_secure(self.CREDS_KEY, creds)
                    
                    return True
                else:
                    typer.echo("❌ Login succeeded but failed to store token securely")
                    return False
            else:
                error_detail = response.json().get('detail', 'Unknown error') if response.content else 'Connection failed'
                typer.echo(f"❌ Login failed: {error_detail}")
                return False
                
        except requests.exceptions.RequestException as e:
            typer.echo(f"❌ Connection error: {e}")
            return False
        except Exception as e:
            typer.echo(f"❌ Login error: {e}")
            return False
    
    def get_auth_headers(self) -> Optional[Dict[str, str]]:
        """Get authentication headers for API calls"""
        token = self._retrieve_secure(self.TOKEN_KEY)
        if token:
            return {"Authorization": f"Bearer {token}"}
        return None
    
    def is_logged_in(self) -> bool:
        """Check if user is logged in"""
        return self.get_auth_headers() is not None
    
    def logout(self, clear_credentials: bool = False) -> bool:
        """Logout and remove stored token"""
        try:
            success = self._delete_secure(self.TOKEN_KEY)
            
            if clear_credentials:
                self._delete_secure(self.CREDS_KEY)
                
            if success:
                typer.echo("✅ Logged out successfully!")
            else:
                typer.echo("⚠️  Logout completed (some data may not have been cleared)")
            return True
        except Exception as e:
            typer.echo(f"❌ Logout error: {e}")
            return False
    
    def refresh_token_if_needed(self) -> bool:
        """Check token validity and refresh if needed"""
        headers = self.get_auth_headers()
        if not headers:
            return False
            
        try:
            # Test token with a lightweight API call
            base_url_str = str(self.base_url)  # Ensure it's a string
            health_url = f"{base_url_str}/healthcheck"
            response = requests.get(health_url, headers=headers, timeout=10)
            
            if response.status_code == 401:
                # Try to auto-renew with stored credentials
                return self._auto_renew_token()
            
            return response.status_code == 200
            
        except Exception:
            return self._auto_renew_token()
    
    def _auto_renew_token(self) -> bool:
        """Attempt to automatically renew token using stored credentials"""
        try:
            creds_data = self._retrieve_secure(self.CREDS_KEY)
            if not creds_data:
                return False
                
            creds = json.loads(creds_data)
            typer.echo("🔄 Token expired, attempting auto-renewal...")
            
            # Clear old token and re-login
            self._delete_secure(self.TOKEN_KEY)
            return self.login(creds["access_key"], creds["access_secret"], save_credentials=False)
            
        except Exception:
            typer.echo("❌ Auto-renewal failed. Please login again.")
            return False
    
    def get_user_info(self) -> Optional[Dict[str, Any]]:
        """Get current user information"""
        headers = self.get_auth_headers()
        if not headers:
            return None
            
        try:
            user_url = f"{self.auth_url}/auth/user/"
            response = requests.get(user_url, headers=headers, timeout=10)

            if response.status_code == 200:
                return response.json()
            return None
            
        except Exception:
            return None
    
    def clear_all_data(self) -> bool:
        """Clear all stored authentication data"""
        try:
            self._delete_secure(self.TOKEN_KEY)
            self._delete_secure(self.CREDS_KEY)
            
            # Also remove any legacy token files
            legacy_files = [
                self.config_dir / "token",
                self.config_dir / "token.json",
            ]
            
            for file_path in legacy_files:
                if file_path.exists():
                    file_path.unlink()
            
            typer.echo("✅ All authentication data cleared!")
            return True
            
        except Exception as e:
            typer.echo(f"❌ Error clearing data: {e}")
            return False


# Global auth manager instances cache
_auth_managers: Dict[str, AuthManager] = {}

def get_auth_manager(base_url: str = None, cluster_name: str = None) -> AuthManager:
    """
    Get an auth manager instance, creating one if needed.
    Instances are cached by cluster_name to ensure singleton behavior per cluster.
    """
    global _auth_managers
    
    # Determine cache key
    if cluster_name:
        key = cluster_name
    elif base_url:
        key = base_url
    else:
        # Check env var for default cluster
        key = os.environ.get("NASIKO_CLUSTER_NAME", "default")
    
    if key not in _auth_managers:
        _auth_managers[key] = AuthManager(base_url=base_url, cluster_name=cluster_name)
        
    return _auth_managers[key]