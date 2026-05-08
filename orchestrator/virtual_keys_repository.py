"""
VirtualKeyRepository — MongoDB storage for per-agent LiteLLM virtual keys.

Schema per document:
  {
    "agent_name":  str,   # unique key (agent identifier)
    "owner_id":    str,
    "virtual_key": str,   # Fernet-encrypted virtual key value
    "key_alias":   str,   # LiteLLM key_alias (e.g. "agent-<name>")
    "created_at":  datetime,
    "rotated_at":  datetime | None,
    "revoked_at":  datetime | None,
    "status":      "active" | "rotated" | "revoked",
  }

Usage from the CLI (litellm_setup.py) and from the orchestrator listener.
Encryption is handled via Fernet using USER_CREDENTIALS_ENCRYPTION_KEY env var,
mirroring the pattern in app/repository/base_repository.py.
"""

import os
import base64
import logging
from datetime import datetime, timezone
from typing import Optional

import cryptography.fernet
import motor.motor_asyncio


def _get_encryption_key() -> bytes:
    """Read Fernet key from USER_CREDENTIALS_ENCRYPTION_KEY env var."""
    env_key = os.getenv("USER_CREDENTIALS_ENCRYPTION_KEY")
    if not env_key:
        raise ValueError(
            "USER_CREDENTIALS_ENCRYPTION_KEY environment variable is required "
            "but not found. Cannot encrypt/decrypt virtual keys."
        )
    key_bytes = env_key.encode()
    cryptography.fernet.Fernet(key_bytes)  # validate — raises if malformed
    return key_bytes


def _encrypt(plaintext: str) -> str:
    """Fernet-encrypt plaintext; returns base64-encoded ciphertext."""
    fernet = cryptography.fernet.Fernet(_get_encryption_key())
    encrypted = fernet.encrypt(plaintext.encode())
    return base64.urlsafe_b64encode(encrypted).decode()


def _decrypt(ciphertext: str) -> str:
    """Fernet-decrypt base64-encoded ciphertext; returns plaintext."""
    encrypted_bytes = base64.urlsafe_b64decode(ciphertext.encode())
    fernet = cryptography.fernet.Fernet(_get_encryption_key())
    return fernet.decrypt(encrypted_bytes).decode()


class VirtualKeyRepository:
    """
    Async MongoDB repository for per-agent LiteLLM virtual keys.

    Instantiate with a Motor database object:
        repo = VirtualKeyRepository(db, logger)

    Or use the factory method with a Mongo URL:
        repo, client = VirtualKeyRepository.from_url(mongo_url, db_name, logger)
        # call client.close() when done
    """

    COLLECTION = "virtual_keys"

    def __init__(self, db, logger: logging.Logger):
        self.collection = db[self.COLLECTION]
        self.logger = logger

    @classmethod
    def from_url(
        cls,
        mongo_url: str,
        db_name: str,
        logger: logging.Logger,
    ) -> tuple["VirtualKeyRepository", motor.motor_asyncio.AsyncIOMotorClient]:
        """Factory: create client + repository from a Mongo connection URL."""
        client = motor.motor_asyncio.AsyncIOMotorClient(mongo_url)
        db = client[db_name]
        return cls(db, logger), client

    # -------------------------------------------------------------------------
    # Write operations
    # -------------------------------------------------------------------------

    async def save_key(
        self,
        agent_name: str,
        owner_id: str,
        virtual_key: str,
        key_alias: str = "",
    ) -> None:
        """Upsert a virtual key record for the given agent (encrypts key value)."""
        doc = {
            "agent_name": agent_name,
            "owner_id": owner_id,
            "virtual_key": _encrypt(virtual_key),
            "key_alias": key_alias or f"agent-{agent_name}",
            "created_at": datetime.now(timezone.utc),
            "rotated_at": None,
            "revoked_at": None,
            "status": "active",
        }
        await self.collection.update_one(
            {"agent_name": agent_name},
            {"$set": doc},
            upsert=True,
        )
        self.logger.info(f"[VirtualKeyRepo] Saved virtual key for agent '{agent_name}'")

    async def mark_rotated(self, agent_name: str, new_key: str) -> None:
        """Replace stored key with new_key; mark rotated_at timestamp."""
        await self.collection.update_one(
            {"agent_name": agent_name},
            {
                "$set": {
                    "virtual_key": _encrypt(new_key),
                    "rotated_at": datetime.now(timezone.utc),
                    "status": "active",
                }
            },
        )
        self.logger.info(
            f"[VirtualKeyRepo] Marked key rotated for agent '{agent_name}'"
        )

    async def mark_revoked(self, agent_name: str, virtual_key: str) -> None:
        """Mark the agent's key as revoked."""
        await self.collection.update_one(
            {"agent_name": agent_name},
            {
                "$set": {
                    "revoked_at": datetime.now(timezone.utc),
                    "status": "revoked",
                }
            },
        )
        self.logger.info(
            f"[VirtualKeyRepo] Marked key revoked for agent '{agent_name}'"
        )

    # -------------------------------------------------------------------------
    # Read operations
    # -------------------------------------------------------------------------

    async def get_active_key(self, agent_name: str) -> Optional[str]:
        """Return decrypted virtual key for agent, or None if not found / revoked."""
        record = await self.collection.find_one(
            {"agent_name": agent_name, "status": "active"}
        )
        if not record:
            return None
        try:
            return _decrypt(record["virtual_key"])
        except Exception as exc:
            self.logger.warning(
                f"[VirtualKeyRepo] Failed to decrypt key for agent '{agent_name}': {exc}"
            )
            return None

    async def get_record(self, agent_name: str) -> Optional[dict]:
        """Return the raw MongoDB document (with decrypted key) or None."""
        record = await self.collection.find_one({"agent_name": agent_name})
        if not record:
            return None
        try:
            record["virtual_key"] = _decrypt(record["virtual_key"])
        except Exception:
            record["virtual_key"] = "<decrypt-error>"
        record.pop("_id", None)  # remove ObjectId for clean dict
        return record

    async def list_all(self) -> list[dict]:
        """Return all virtual key records (keys masked)."""
        results = []
        async for doc in self.collection.find({}):
            doc.pop("_id", None)
            key_val = doc.get("virtual_key", "")
            if key_val:
                try:
                    plain = _decrypt(key_val)
                    # Mask: show first 8 + ... + last 4 chars
                    doc["virtual_key"] = plain[:8] + "..." + plain[-4:] if len(plain) > 12 else "***"
                except Exception:
                    doc["virtual_key"] = "<decrypt-error>"
            results.append(doc)
        return results
