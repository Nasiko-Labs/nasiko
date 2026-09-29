"""Original version-pinned vLLM scheduler extension, loaded only on the GPU host."""
import importlib.metadata
import json
import os
from pathlib import Path
import time

from vllm.v1.core.sched.async_scheduler import AsyncScheduler
from agentkv.retention import ResidentGroup, prioritize, block_hashes


class AgentKVScheduler(AsyncScheduler):
    def __init__(self, *args, **kwargs):
        if importlib.metadata.version("vllm") != "0.29.0":
            raise RuntimeError("AgentKV adapter requires vLLM 0.29.0")
        super().__init__(*args, **kwargs)
        self.agentkv_groups = {}
        self.agentkv_directory = Path(os.environ.get("AGENTKV_ENGINE_DIR", "/tmp/agentkv-engine"))
        self.agentkv_directory.mkdir(mode=0o700, exist_ok=True)
        self.agentkv_last_write = 0.0
        self.agentkv_acks = []
        self.agentkv_published_acks = []
        self.agentkv_action_history = []
        self.agentkv_previous_actions = set()
        self.agentkv_disabled = False
        self.agentkv_steps = 0
        self.agentkv_overhead_seconds = 0.0
        self.agentkv_hints = []
        self.agentkv_last_read = 0.0

    def _free_request_blocks(self, request):
        # Only the parent frees/defer-frees allocations. The cache lookup below
        # observes the jointly reusable boundary, including detached Mamba checkpoints.
        super()._free_request_blocks(request)
        captured = {}
        try:
            allocation, reusable_tokens, _ = self.kv_cache_manager.get_computed_blocks(request)
            pool = self.kv_cache_manager.block_pool
            reusable_groups = 0
            for blocks in allocation.blocks:
                reusable = [b for b in blocks if not b.is_null and block_hashes(pool,b.block_id)]
                if reusable:
                    reusable_groups += 1
                for block in reusable:
                    captured[block.block_id] = block_hashes(pool,block.block_id)
            if reusable_tokens and reusable_groups == len(allocation.blocks) and captured:
                self.agentkv_groups[request.request_id] = ResidentGroup(request.request_id, captured, reusable_tokens)
        except Exception:
            self.agentkv_disabled = True
        self._agentkv_publish(time.time())

    def schedule(self, throttle_prefills=False):
        started = time.perf_counter()
        now = time.time()
        if not self.agentkv_disabled:
            try:
                if now - self.agentkv_last_read >= .05:
                    file = self.agentkv_directory / "hints.json"
                    payload = json.loads(file.read_text()) if file.exists() else {}
                    self.agentkv_hints = payload.get("hints", []) if payload.get("enabled") else []
                    self.agentkv_last_read = now
                if self.agentkv_hints:
                    self.agentkv_acks = prioritize(self.kv_cache_manager.block_pool,
                                                   self.agentkv_groups, self.agentkv_hints, now)
                else:
                    self.agentkv_acks = []
            except Exception:
                self.agentkv_disabled = True
                self.agentkv_acks = []
        actions={(a['request_id'],a['expires_at']) for a in self.agentkv_acks if a['status']=='prioritized'}
        for ack in self.agentkv_acks:
            if ack['status']=='prioritized' and (ack['request_id'],ack['expires_at']) not in self.agentkv_previous_actions:
                self.agentkv_action_history.append({**ack,'timestamp':now})
        self.agentkv_action_history=self.agentkv_action_history[-256:]
        self.agentkv_previous_actions=actions
        self.agentkv_steps += 1
        self.agentkv_overhead_seconds += time.perf_counter() - started
        if now - self.agentkv_last_write >= .2 or self.agentkv_acks != self.agentkv_published_acks:
            self._agentkv_publish(now)
        return super().schedule(throttle_prefills=throttle_prefills)

    def _agentkv_publish(self, now):
        pool = self.kv_cache_manager.block_pool
        self.agentkv_groups = {key: group for key, group in list(self.agentkv_groups.items())[-256:]
            if all(hashes.issubset(block_hashes(pool,block))
                   for block, hashes in group.blocks.items())}
        state = {"adapter": "eviction-priority-v1", "disabled": self.agentkv_disabled,
            "timestamp": now, "total_blocks": pool.num_gpu_blocks,
            "free_blocks": pool.free_block_queue.num_free_blocks,
            "bytes_per_block": max((t.size for t in self.kv_cache_config.kv_cache_tensors), default=0) // pool.num_gpu_blocks,
            "token_block_size": self.block_size,
            "groups": [{"request_id": key, "blocks": len(group.blocks), "reusable_tokens":group.reusable_tokens}
                       for key, group in self.agentkv_groups.items()],
            "acknowledgements": self.agentkv_acks, "action_history":self.agentkv_action_history,
            "schedule_steps": self.agentkv_steps, "overhead_seconds": self.agentkv_overhead_seconds}
        try:
            tmp = self.agentkv_directory / "state.tmp"
            tmp.write_text(json.dumps(state))
            tmp.replace(self.agentkv_directory / "state.json")
        except OSError:
            pass
        self.agentkv_last_write = now
        self.agentkv_published_acks = self.agentkv_acks
