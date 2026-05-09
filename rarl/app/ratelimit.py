import time


class TokenBucket:
    def __init__(self, rate_per_sec: float, burst: int) -> None:
        self.rate = float(rate_per_sec)
        self.capacity = int(burst)
        self.tokens = float(burst)
        self.updated = time.monotonic()

    def _refill(self) -> None:
        now = time.monotonic()
        elapsed = now - self.updated
        self.tokens = min(self.capacity, self.tokens + elapsed * self.rate)
        self.updated = now

    def try_acquire(self, n: int = 1) -> bool:
        self._refill()
        if self.tokens >= n:
            self.tokens -= n
            return True
        return False

    def time_until_available(self, n: int = 1) -> float:
        self._refill()
        if self.tokens >= n:
            return 0.0
        return (n - self.tokens) / self.rate
