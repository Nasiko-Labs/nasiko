from pydantic_settings import BaseSettings
from pathlib import Path

class Config(BaseSettings):
    ENV: str = "development"
    MONGO_NASIKO_USER: str = "admin"
    MONGO_AUTH_SOURCE: str = "admin"  
    MONGO_NASIKO_PASSWORD: str = "password"
    MONGO_NASIKO_HOST: str = "localhost"
    MONGO_NASIKO_PORT: str = "27017"
    MONGO_NASIKO_DATABASE: str = "nasiko"
    REDIS_HOST: str = "localhost"
    REDIS_PORT: int = 6379
    REDIS_DB: int = 0
    PHOENIX_SERVICE_URL: str = "http://phoenix-service.nasiko.svc.cluster.local:6006"
    OPENAI_API_KEY: str = "empty key"
    BUILDKIT_ADDRESS: str = "tcp://buildkitd.buildkit.svc.cluster.local:1234"
    REGISTRY_URL: str = ""  # Set via environment variable during deployment
    GATEWAY_URL: str = ""  # Public gateway URL (e.g., http://<gateway-ip>)
    DO_TOKEN: str = "empty token"

    # Local docker deployments don't require Kubernetes access
    K8S_ENABLED: bool = True

    # Internal base URL used for orchestrator callbacks
    NASIKO_API_URL: str = "http://nasiko-backend:8000"

    GITHUB_CLIENT_ID: str = "Ov23liTKuIzo8VjY8ODP"
    GITHUB_CLIENT_SECRET: str = "1345643db25706c291e39765add04dfe85d51a68"
    GITHUB_REDIRECT_BASE_URL: str = "http://localhost:8000"

    # Encryption for sensitive data like N8N API keys
    USER_CREDENTIALS_ENCRYPTION_KEY: str = ""

    @property
    def MONGO_URI(self) -> str:
        return f"mongodb://{self.MONGO_NASIKO_USER}:{self.MONGO_NASIKO_PASSWORD}@{self.MONGO_NASIKO_HOST}:{self.MONGO_NASIKO_PORT}/{self.MONGO_NASIKO_DATABASE}?authSource={self.MONGO_AUTH_SOURCE}"
    
    @property
    def MONGO_DB(self) -> str:
        return self.MONGO_NASIKO_DATABASE
    
    model_config = {
        "env_file": [".env", "app/.env"],
        "env_file_encoding": "utf-8",
        "case_sensitive": False
    }

settings = Config()
