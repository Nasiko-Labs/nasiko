import redis
import os

REDIS_HOST = os.getenv("REDIS_HOST", "localhost")
REDIS_PORT = int(os.getenv("REDIS_PORT", 6379))

def get_redis_client():
    return redis.Redis(host=REDIS_HOST, port=REDIS_PORT, db=0)

def set_github_access_token(token: str):
    r = get_redis_client()
    r.set("github_access_token", token)

def get_github_access_token():
    r = get_redis_client()
    token = r.get("github_access_token")
    return token.decode("utf-8") if token else None

def clear_github_access_token():
    r = get_redis_client()
    r.delete("github_access_token")
