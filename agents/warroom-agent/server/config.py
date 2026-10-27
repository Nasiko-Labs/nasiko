import os
from pathlib import Path
from dotenv import load_dotenv

# Load .env file from project root if present
env_path = Path(__file__).resolve().parent.parent / ".env"
load_dotenv(dotenv_path=env_path)


class Settings:
    anakin_api_key: str | None
    dronahq_webhook_url: str | None
    dronahq_api_key: str | None
    anakin_limit: int
    port: int

    def __init__(self):
        self.reload()

    def reload(self):
        self.anakin_api_key = os.getenv("ANAKIN_API_KEY")
        self.dronahq_webhook_url = os.getenv("DRONAHQ_WEBHOOK_URL")
        self.dronahq_api_key = os.getenv("DRONAHQ_API_KEY")
        try:
            self.anakin_limit = int(os.getenv("ANAKIN_LIMIT", "3"))
        except ValueError:
            self.anakin_limit = 3
        try:
            self.port = int(os.getenv("PORT", "8000"))
        except ValueError:
            self.port = 8000

    def is_anakin_configured(self) -> bool:
        return bool(self.anakin_api_key and self.anakin_api_key.strip())

    def is_dronahq_configured(self) -> bool:
        return bool(
            self.dronahq_webhook_url
            and self.dronahq_webhook_url.strip()
            and self.dronahq_api_key
            and self.dronahq_api_key.strip()
        )


settings = Settings()
